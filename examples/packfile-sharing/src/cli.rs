use clap::Parser;
use std::path::PathBuf;

use libp2p::Multiaddr;

#[derive(Parser, Debug)]
#[clap(name = "libp2p file sharing example")]
struct Opt {
    /// Fixed value to generate deterministic peer ID.
    #[clap(long, default_value = "0")]
    secret_key_seed: Option<u8>,

    #[clap(long)]
    peer: Option<Multiaddr>,

    #[clap(long)]
    listen_address: Option<Multiaddr>,

    #[clap(subcommand)]
    argument: CliArgument,
}

#[derive(Debug, Parser)]
enum CliArgument {
    Provide {
        #[clap(long)]
        path: PathBuf,
        #[clap(long)]
        name: String,
    },
    Get {
        #[clap(long)]
        name: String,
    },
}
