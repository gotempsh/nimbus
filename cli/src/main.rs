//! `nimbus` — a thin CLI over `nimbus_cloud::CloudProvider`.
//!
//! Built to be driven by a program as readily as by a person: results are
//! JSON on stdout, failures are JSON on stderr with a `hint` saying what to
//! do next, and no credential value is ever written to either (see
//! `output`). Credentials come from the environment, the OS credential store
//! or an owner-only file, in that order (see `credentials`).

mod cli;
mod commands;
mod credentials;
mod login;
mod output;
mod store_file;
mod store_os;

use clap::Parser;
use cli::{Cli, Command, InstanceCmd, NetworkCmd, VolumeCmd};
use commands::{
    api_error, build_provider, providers_report, require_provider, resolve_error, VerifyReport,
};
use credentials::{resolve, CredentialStore, Source, Sources};
use login::{StoreMode, Stores, TerminalPrompter};
use nimbus_cloud::{CloudProvider, CreateInstance, CreateNetwork, CreateVolume, ProviderInfo};
use output::{print_error, print_json, print_line, CliError};
use std::io::{IsTerminal, Read};
use std::path::Path;
use std::process::ExitCode;
use std::sync::Arc;
use store_file::FileStore;
use store_os::OsStore;

/// Reads an environment variable; unset, empty and non-UTF-8 are all "not
/// set".
fn env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.is_empty())
}

/// The persistent stores this run may use, built once from the store mode.
struct OpenStores {
    os: Option<OsStore>,
    file: Option<FileStore>,
}

impl OpenStores {
    fn open(mode: StoreMode) -> Result<Self, CliError> {
        let file = match mode {
            StoreMode::Os => None,
            StoreMode::Auto | StoreMode::File => {
                let dir = store_file::config_dir(&env).map_err(|e| {
                    CliError::new(
                        "invalid_configuration",
                        e.to_string(),
                        "Set NIMBUS_CONFIG_DIR to a directory nimbus may keep its credentials file in.",
                    )
                })?;
                Some(FileStore::in_dir(&dir))
            }
        };
        let os = match mode {
            StoreMode::File => None,
            StoreMode::Auto | StoreMode::Os => Some(OsStore),
        };
        Ok(Self { os, file })
    }

    fn stores(&self) -> Stores<'_> {
        Stores {
            os: self.os.as_ref().map(|s| s as &dyn CredentialStore),
            file: self.file.as_ref().map(|s| s as &dyn CredentialStore),
        }
    }
}

fn store_mode() -> Result<StoreMode, CliError> {
    match env("NIMBUS_CREDENTIAL_STORE") {
        Some(value) => StoreMode::parse(&value),
        None => Ok(StoreMode::Auto),
    }
}

/// Reads a credential for `login` from a file or standard input.
fn read_credential_input(from_file: Option<&Path>) -> Result<String, CliError> {
    match from_file {
        Some(path) => std::fs::read_to_string(path).map_err(|e| {
            CliError::new(
                "unreadable_file",
                format!("cannot read {}: {e}", path.display()),
                "Check the path passed to --from-file and that the file is readable text.",
            )
        }),
        None => {
            let mut input = String::new();
            std::io::stdin().read_to_string(&mut input).map_err(|e| {
                CliError::new(
                    "unreadable_input",
                    format!("cannot read the credential from standard input: {e}"),
                    "Pipe the credential in as text, or use --from-file <path>.",
                )
            })?;
            Ok(input)
        }
    }
}

fn run_login(
    info: &'static ProviderInfo,
    from_file: Option<&Path>,
    stdin: bool,
    store: Option<StoreMode>,
) -> Result<ExitCode, CliError> {
    let mode = match store {
        Some(mode) => mode,
        None => store_mode()?,
    };
    let fields = if from_file.is_some() || stdin {
        let input = read_credential_input(from_file)?;
        login::parse_input(info, &input)?
    } else {
        if !std::io::stdin().is_terminal() {
            return Err(CliError::new(
                "no_terminal",
                "there is no terminal to prompt on, so the credential cannot be typed in",
                format!(
                    "Ask the person to run `{}` in their own terminal: it prompts for the secret with echo off. For unattended use, pass --from-file <path> or --stdin instead.",
                    credentials::login_command(info)
                ),
            )
            .for_provider(info.id));
        }
        login::collect_interactive(info, &mut TerminalPrompter)?
    };
    for field in info.credential_fields.iter().filter(|f| f.secret) {
        if let Some(value) = fields.get(field.name) {
            output::register_secret(value.expose());
        }
    }
    let open = OpenStores::open(mode)?;
    let report = login::store_credential(info, fields, &open.stores(), &env)?;
    print_json(&report);
    Ok(ExitCode::SUCCESS)
}

/// Reads the first-boot script for `instance create --user-data-file`.
fn read_user_data(path: &Path) -> Result<String, CliError> {
    let hint = "Pass --user-data-file the path of a readable UTF-8 text file holding the first-boot script.";
    let text = std::fs::read_to_string(path).map_err(|e| {
        CliError::new(
            "unreadable_file",
            format!("cannot read the user data file {}: {e}", path.display()),
            hint,
        )
    })?;
    if text.trim().is_empty() {
        return Err(CliError::new(
            "invalid_input",
            format!("the user data file {} is empty", path.display()),
            hint,
        ));
    }
    Ok(text)
}

/// The commands that talk to a provider.
async fn run_api(
    command: Command,
    info: &'static ProviderInfo,
    source: Source,
    provider: Arc<dyn CloudProvider>,
) -> Result<ExitCode, CliError> {
    let api = |e| api_error(info, source, e);
    match command {
        Command::Verify => {
            let check = provider.check_credential().await.map_err(api)?;
            let report = VerifyReport::new(info, source, check);
            print_json(&report);
            // The report is the answer either way; the status lets a script
            // stop before trying to create with a credential that cannot.
            return Ok(if report.usable() {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            });
        }
        Command::Regions => print_json(&provider.regions().await.map_err(api)?),
        Command::Sizes { region } => {
            print_json(&provider.instance_types(&region).await.map_err(api)?)
        }
        Command::Images { region } => print_json(&provider.images(&region).await.map_err(api)?),
        Command::Instance(cmd) => match cmd {
            InstanceCmd::List => print_json(&provider.list_instances().await.map_err(api)?),
            InstanceCmd::Get { id } => print_json(&provider.get_instance(&id).await.map_err(api)?),
            InstanceCmd::Create {
                name,
                region,
                r#type,
                image,
                ssh_key,
                network,
                user_data_file,
            } => {
                // Read before anything is sent: a bad path must not surface
                // after an SSH key has already been uploaded.
                let user_data = user_data_file.as_deref().map(read_user_data).transpose()?;
                let ssh_public_key = std::fs::read_to_string(&ssh_key).unwrap_or(ssh_key);
                print_json(
                    &provider
                        .create_instance(CreateInstance {
                            name,
                            region,
                            instance_type: r#type,
                            image,
                            ssh_public_key,
                            network_id: network,
                            user_data,
                        })
                        .await
                        .map_err(api)?,
                )
            }
            InstanceCmd::Delete { id } => {
                provider.delete_instance(&id).await.map_err(api)?;
                print_line(&format!("deleted {id}"));
            }
        },
        Command::Volume(cmd) => match cmd {
            VolumeCmd::List => print_json(&provider.list_volumes().await.map_err(api)?),
            VolumeCmd::Create {
                name,
                region,
                size_gb,
                instance,
            } => print_json(
                &provider
                    .create_volume(CreateVolume {
                        name,
                        region,
                        size_gb,
                        instance_id: instance,
                    })
                    .await
                    .map_err(api)?,
            ),
            VolumeCmd::Attach {
                volume_id,
                instance_id,
            } => {
                provider
                    .attach_volume(&volume_id, &instance_id)
                    .await
                    .map_err(api)?;
                print_line(&format!("attached {volume_id} to {instance_id}"));
            }
            VolumeCmd::Detach { volume_id } => {
                provider.detach_volume(&volume_id).await.map_err(api)?;
                print_line(&format!("detached {volume_id}"));
            }
            VolumeCmd::Delete { id } => {
                provider.delete_volume(&id).await.map_err(api)?;
                print_line(&format!("deleted {id}"));
            }
        },
        Command::Network(cmd) => match cmd {
            NetworkCmd::List => print_json(&provider.list_networks().await.map_err(api)?),
            NetworkCmd::Create { name, region, cidr } => print_json(
                &provider
                    .create_network(CreateNetwork {
                        name,
                        region,
                        ip_range: cidr,
                    })
                    .await
                    .map_err(api)?,
            ),
            NetworkCmd::Delete { id } => {
                provider.delete_network(&id).await.map_err(api)?;
                print_line(&format!("deleted {id}"));
            }
        },
        Command::Providers | Command::Login { .. } | Command::Logout => {
            unreachable!("handled before a provider is built")
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn run(cli: Cli) -> Result<ExitCode, CliError> {
    // Credential stores are consulted here, before any async runtime exists:
    // the OS store's client blocks, which a runtime thread must not do.
    match cli.command {
        Command::Providers => {
            let open = OpenStores::open(store_mode()?)?;
            let sources = Sources {
                env: &env,
                stores: open.stores().in_read_order(),
            };
            print_json(&providers_report(&sources));
            Ok(ExitCode::SUCCESS)
        }
        Command::Login {
            from_file,
            stdin,
            store,
        } => {
            let info = require_provider(cli.provider.as_deref())?;
            run_login(info, from_file.as_deref(), stdin, store)
        }
        Command::Logout => {
            let info = require_provider(cli.provider.as_deref())?;
            let open = OpenStores::open(store_mode()?)?;
            let report = login::remove_credential(info, &open.stores(), &env)?;
            print_json(&report);
            Ok(ExitCode::SUCCESS)
        }
        command => {
            let info = require_provider(cli.provider.as_deref())?;
            let open = OpenStores::open(store_mode()?)?;
            let sources = Sources {
                env: &env,
                stores: open.stores().in_read_order(),
            };
            let creds = resolve(info, &sources).map_err(|e| resolve_error(info, e))?;
            for secret in creds.secrets(info) {
                output::register_secret(&secret);
            }
            let provider = build_provider(info, &creds, cli.base_url)?;
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .map_err(|e| {
                    CliError::new(
                        "internal",
                        format!("could not start the async runtime: {e}"),
                        "Run the same command again; if it keeps failing, report it as a bug.",
                    )
                })?;
            runtime.block_on(run_api(command, info, creds.source, provider))
        }
    }
}

/// Reports a command line clap could not parse.
///
/// Help and version requests are clap's own output. Anything else is a
/// failure like any other, so it takes the same JSON shape — with the exit
/// status clap has always used for usage errors.
fn usage_error(error: clap::Error) -> ExitCode {
    use clap::error::ErrorKind;
    if matches!(
        error.kind(),
        ErrorKind::DisplayHelp
            | ErrorKind::DisplayVersion
            | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
    ) {
        error.exit();
    }
    let is_login = std::env::args().skip(1).any(|arg| arg == "login");
    let report = if is_login && error.kind() == ErrorKind::UnknownArgument {
        // The unexpected argument may well be the credential itself, typed
        // where a value is never accepted — so it is not quoted back.
        CliError::new(
            "usage",
            "`nimbus login` was given an argument it does not accept",
            "A credential is never passed on the command line. Run `nimbus login --provider <id>` and type it at the prompt, or pass --from-file <path> or --stdin.",
        )
    } else {
        let rendered = error.render().to_string();
        // The first paragraph is the complaint (which may list several
        // arguments, one per line); what follows is tips and usage.
        let complaint: Vec<&str> = rendered
            .lines()
            .take_while(|line| !line.trim().is_empty())
            .map(str::trim)
            .collect();
        let message = match complaint.join(" ").trim_start_matches("error: ") {
            "" => "the command line could not be understood".to_owned(),
            text => text.to_owned(),
        };
        let hint = match rendered.lines().find(|line| line.starts_with("Usage: ")) {
            Some(usage) => format!("{usage} — add --help to the command to see every argument."),
            None => "Run `nimbus --help` to see the commands and their arguments.".to_owned(),
        };
        CliError::new("usage", message, hint)
    };
    print_error(&report);
    ExitCode::from(2)
}

fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => return usage_error(error),
    };
    match run(cli) {
        Ok(code) => code,
        Err(error) => {
            print_error(&error);
            ExitCode::FAILURE
        }
    }
}
