//! Shows what a set of targets resolves to, and keeps showing it while pods
//! come and go. A way to try target resolution on its own.
//!
//! ```text
//! cargo run -p gossip-core --example resolve -- [--context NAME] [--container GLOB] TARGET...
//! ```
//!
//! `+` is a container to follow (`at start` when it was there when the
//! example started), `~` a new state of one, `-` one that is gone, and `--`
//! a notice. Stop it with Ctrl-C.

use std::error::Error;

use gossip_core::{Cluster, ContainerEvent, Pattern, Resolver, ResolverEvent, Target};

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("resolve: {error}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), Box<dyn Error>> {
    let mut context = None;
    let mut containers = None;
    let mut targets = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--context" => context = Some(args.next().ok_or("--context needs a name")?),
            "--container" => {
                let glob = args.next().ok_or("--container needs a glob")?;
                containers = Some(glob.parse::<Pattern>()?);
            }
            target => targets.push(target.parse::<Target>()?),
        }
    }
    if targets.is_empty() {
        return Err("usage: resolve [--context NAME] [--container GLOB] TARGET...".into());
    }

    let cluster = Cluster::open(context.as_deref()).await?;
    cluster.check().await?;
    println!("-- resolving against the cluster {}", cluster.name());

    let mut resolver = Resolver::start(&cluster, targets, containers, None);
    while let Some(event) = resolver.next().await {
        match event {
            ResolverEvent::Container(ContainerEvent::Added { id, view, initial }) => println!(
                "+ {}/{}/{}  {}, {} restarts{}",
                id.namespace,
                id.pod,
                id.container,
                view.state,
                view.restart_count,
                if initial { "  (at start)" } else { "" }
            ),
            ResolverEvent::Container(ContainerEvent::Changed { id, view }) => println!(
                "~ {}/{}/{}  {}, {} restarts",
                id.namespace, id.pod, id.container, view.state, view.restart_count
            ),
            ResolverEvent::Container(ContainerEvent::Removed { id }) => {
                println!("- {}/{}/{}", id.namespace, id.pod, id.container);
            }
            ResolverEvent::Cluster(event) => {
                println!("! {} {}: {}", event.regarding, event.reason, event.message);
            }
            ResolverEvent::Notice(notice) => println!("-- {}", notice.message),
            ResolverEvent::Matched { namespace, pods } => {
                let total: usize = pods.values().sum();
                println!("= {namespace}: {total} pods followed");
            }
            ResolverEvent::Settled => println!("-- everything that was there is listed"),
        }
    }
    Ok(())
}
