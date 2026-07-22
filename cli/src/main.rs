use anyhow::{anyhow, Result};
use clap::{Parser, Subcommand};
use nimbus_cloud::{
    providers::{Hetzner, Ovh, OvhRegion, Vultr},
    CloudProvider, CreateInstance, CreateNetwork, CreateVolume,
};
use std::sync::Arc;

#[derive(Parser)]
#[command(name = "nimbus", about = "One CLI for instance/storage/network provisioning across clouds")]
struct Cli {
    /// hetzner | vultr | ovh
    #[arg(long, global = true, env = "NIMBUS_PROVIDER")]
    provider: Option<String>,

    /// Override the provider's API root — e.g. http://127.0.0.1:8090/v1 to
    /// target `nimbus-mock` instead of the real Hetzner/Vultr/OVH API.
    #[arg(long, global = true, env = "NIMBUS_BASE_URL")]
    base_url: Option<String>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// List regions available for the provider.
    Regions,
    /// List instance types (with monthly price) for a region.
    Sizes { region: String },
    #[command(subcommand)]
    Instance(InstanceCmd),
    #[command(subcommand)]
    Volume(VolumeCmd),
    #[command(subcommand)]
    Network(NetworkCmd),
}

#[derive(Subcommand)]
enum InstanceCmd {
    List,
    Get { id: String },
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
    },
    Delete { id: String },
}

#[derive(Subcommand)]
enum VolumeCmd {
    List,
    Create {
        name: String,
        region: String,
        #[arg(long)]
        size_gb: u32,
        #[arg(long)]
        instance: Option<String>,
    },
    Attach { volume_id: String, instance_id: String },
    Detach { volume_id: String },
    Delete { id: String },
}

#[derive(Subcommand)]
enum NetworkCmd {
    List,
    Create {
        name: String,
        region: String,
        #[arg(long)]
        cidr: String,
    },
    Delete { id: String },
}

fn env(key: &str) -> Result<String> {
    std::env::var(key).map_err(|_| anyhow!("missing required env var {key}"))
}

fn build_provider(id: &str, base_url: Option<String>) -> Result<Arc<dyn CloudProvider>> {
    Ok(match id {
        "hetzner" => {
            let p = Hetzner::new(env("HCLOUD_TOKEN")?);
            Arc::new(if let Some(b) = base_url { p.with_base_url(b) } else { p })
        }
        "vultr" => {
            let p = Vultr::new(env("VULTR_API_KEY")?);
            Arc::new(if let Some(b) = base_url { p.with_base_url(b) } else { p })
        }
        "ovh" => {
            let p = Ovh::new(
                OvhRegion::Eu,
                env("OVH_APPLICATION_KEY")?,
                env("OVH_APPLICATION_SECRET")?,
                env("OVH_CONSUMER_KEY")?,
                env("OVH_PROJECT_ID")?,
            );
            Arc::new(if let Some(b) = base_url { p.with_base_url(b) } else { p })
        }
        other => return Err(anyhow!("unknown provider '{other}' (expected hetzner, vultr, or ovh)")),
    })
}

fn print_json<T: serde::Serialize>(v: &T) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(v)?);
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let provider_id = cli.provider.ok_or_else(|| anyhow!("--provider is required (hetzner, vultr, or ovh)"))?;
    let provider = build_provider(&provider_id, cli.base_url)?;

    match cli.command {
        Command::Regions => print_json(&provider.regions().await?)?,
        Command::Sizes { region } => print_json(&provider.instance_types(&region).await?)?,
        Command::Instance(cmd) => match cmd {
            InstanceCmd::List => print_json(&provider.list_instances().await?)?,
            InstanceCmd::Get { id } => print_json(&provider.get_instance(&id).await?)?,
            InstanceCmd::Create { name, region, r#type, image, ssh_key, network } => {
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
                            user_data: None,
                        })
                        .await?,
                )?
            }
            InstanceCmd::Delete { id } => {
                provider.delete_instance(&id).await?;
                println!("deleted {id}");
            }
        },
        Command::Volume(cmd) => match cmd {
            VolumeCmd::List => print_json(&provider.list_volumes().await?)?,
            VolumeCmd::Create { name, region, size_gb, instance } => print_json(
                &provider.create_volume(CreateVolume { name, region, size_gb, instance_id: instance }).await?,
            )?,
            VolumeCmd::Attach { volume_id, instance_id } => {
                provider.attach_volume(&volume_id, &instance_id).await?;
                println!("attached {volume_id} to {instance_id}");
            }
            VolumeCmd::Detach { volume_id } => {
                provider.detach_volume(&volume_id).await?;
                println!("detached {volume_id}");
            }
            VolumeCmd::Delete { id } => {
                provider.delete_volume(&id).await?;
                println!("deleted {id}");
            }
        },
        Command::Network(cmd) => match cmd {
            NetworkCmd::List => print_json(&provider.list_networks().await?)?,
            NetworkCmd::Create { name, region, cidr } => {
                print_json(&provider.create_network(CreateNetwork { name, region, ip_range: cidr }).await?)?
            }
            NetworkCmd::Delete { id } => {
                provider.delete_network(&id).await?;
                println!("deleted {id}");
            }
        },
    }
    Ok(())
}
