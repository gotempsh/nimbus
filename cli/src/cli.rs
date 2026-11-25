//! Command-line definition. There is, on purpose, no option anywhere that
//! takes a credential value: arguments are visible in the process list and
//! saved in shell history.

use crate::login::StoreMode;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "nimbus",
    version,
    about = "One CLI for instance/storage/network provisioning across clouds"
)]
pub struct Cli {
    /// hetzner | vultr | ovh | digitalocean | scaleway | linode | hostinger
    #[arg(long, global = true, env = "NIMBUS_PROVIDER")]
    pub provider: Option<String>,

    /// Override the provider's API root — e.g. http://127.0.0.1:8090/v1 to
    /// target `nimbus-mock` instead of the real Hetzner/Vultr/OVH API.
    #[arg(long, global = true, env = "NIMBUS_BASE_URL")]
    pub base_url: Option<String>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// List every supported provider: the credential fields it needs,
    /// whether credentials are available (and from where), and its caveats.
    /// Needs no credentials.
    Providers,
    /// Store a provider's credentials. Prompts on the terminal with secret
    /// input hidden, unless --from-file or --stdin is given.
    Login {
        /// Read the credential from this file instead of prompting: the
        /// value alone, or one NAME=value line per field.
        #[arg(long, value_name = "PATH", conflicts_with = "stdin")]
        from_file: Option<PathBuf>,
        /// Read the credential from standard input instead of prompting
        /// (same format as --from-file).
        #[arg(long)]
        stdin: bool,
        /// Where to keep it. Default: the OS credential store, or the
        /// owner-only credentials file where there is none. Overrides
        /// NIMBUS_CREDENTIAL_STORE.
        #[arg(long, value_enum, value_name = "STORE")]
        store: Option<StoreMode>,
    },
    /// Remove a provider's stored credentials.
    Logout,
    /// Check the credential without creating anything: does it
    /// authenticate, and can it create instances (yes / no / unknown)?
    Verify,
    /// List regions available for the provider.
    Regions,
    /// List instance types (with monthly price) for a region.
    Sizes { region: String },
    /// List OS images available in a region; `id` is what `instance create
    /// --image` takes.
    Images { region: String },
    /// Create, inspect and delete instances (servers).
    #[command(subcommand)]
    Instance(InstanceCmd),
    /// Create, attach, detach and delete block-storage volumes.
    #[command(subcommand)]
    Volume(VolumeCmd),
    /// Create and delete private networks.
    #[command(subcommand)]
    Network(NetworkCmd),
}

#[derive(Subcommand)]
pub enum InstanceCmd {
    List,
    Get {
        id: String,
    },
    Create {
        name: String,
        region: String,
        #[arg(long)]
        r#type: String,
        #[arg(long)]
        image: String,
        #[arg(long)]
        ssh_key: String,
        #[arg(long)]
        network: Option<String>,
        /// File holding the first-boot script (cloud-init user data) to run
        /// on the new instance. A file rather than a value so that a
        /// multi-line script, or a single-use code inside it, stays out of
        /// shell history.
        #[arg(long, value_name = "PATH")]
        user_data_file: Option<PathBuf>,
    },
    Delete {
        id: String,
    },
}

#[derive(Subcommand)]
pub enum VolumeCmd {
    List,
    Create {
        name: String,
        region: String,
        #[arg(long)]
        size_gb: u32,
        #[arg(long)]
        instance: Option<String>,
    },
    Attach {
        volume_id: String,
        instance_id: String,
    },
    Detach {
        volume_id: String,
    },
    Delete {
        id: String,
    },
}

#[derive(Subcommand)]
pub enum NetworkCmd {
    List,
    Create {
        name: String,
        region: String,
        #[arg(long)]
        cidr: String,
    },
    Delete {
        id: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn definition_is_consistent() {
        Cli::command().debug_assert();
    }

    /// The guarantee that matters most here: there is no way to hand a
    /// credential over as an argument value. `login` takes a path, a switch
    /// and a store name — nothing else, and no positional.
    #[test]
    fn login_has_no_option_that_takes_a_credential_value() {
        let command = Cli::command();
        let login = command
            .get_subcommands()
            .find(|c| c.get_name() == "login")
            .expect("login exists");
        let mut options: Vec<&str> = login
            .get_arguments()
            .filter(|a| !a.is_global_set())
            .map(|a| a.get_id().as_str())
            .collect();
        options.sort_unstable();
        assert_eq!(options, vec!["from_file", "stdin", "store"]);
        assert_eq!(login.get_positionals().count(), 0);

        for attempt in [
            vec![
                "nimbus",
                "login",
                "--provider",
                "hetzner",
                "--token",
                "fake-token",
            ],
            vec!["nimbus", "login", "--provider", "hetzner", "fake-token"],
            vec![
                "nimbus",
                "login",
                "--provider",
                "hetzner",
                "--stdin",
                "fake-token",
            ],
            vec![
                "nimbus",
                "--provider",
                "hetzner",
                "--token",
                "fake-token",
                "regions",
            ],
        ] {
            assert!(
                Cli::try_parse_from(&attempt).is_err(),
                "must be rejected: {attempt:?}"
            );
        }
    }

    #[test]
    fn existing_invocations_still_parse() {
        for args in [
            vec!["nimbus", "--provider", "hetzner", "regions"],
            vec!["nimbus", "--provider", "hetzner", "sizes", "fsn1"],
            vec![
                "nimbus",
                "--provider",
                "hetzner",
                "instance",
                "create",
                "web-1",
                "fsn1",
                "--type",
                "cx22",
                "--image",
                "ubuntu-24.04",
                "--ssh-key",
                "key.pub",
            ],
            vec![
                "nimbus",
                "--provider",
                "hetzner",
                "--base-url",
                "http://127.0.0.1:1/v1",
                "instance",
                "list",
            ],
            vec!["nimbus", "instance", "delete", "42", "--provider", "vultr"],
            vec![
                "nimbus",
                "--provider",
                "ovh",
                "volume",
                "create",
                "v",
                "GRA",
                "--size-gb",
                "10",
            ],
            vec![
                "nimbus",
                "--provider",
                "ovh",
                "network",
                "create",
                "n",
                "GRA",
                "--cidr",
                "10.0.0.0/16",
            ],
        ] {
            assert!(Cli::try_parse_from(&args).is_ok(), "must parse: {args:?}");
        }
    }
}
