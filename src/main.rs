#[macro_use]
extern crate tokio;
extern crate clap;
use clap::App;
use tokio::prelude::*;
use tokio::io::copy;
use tokio::net::TcpListener;

#[derive(Debug)]
struct Conf {
    config: String,
    p2p: String,
    rpc: String,
    peers: Vec<String>
}
fn main() {
    let conf = parse_args();
    println!("{:?}", conf);
    start_p2p(conf);
}

fn parse_args()  -> Conf {
    let yaml = load_yaml!("cli.yml");
    let matches = App::from_yaml(yaml).get_matches();

    let config = matches.value_of("config").unwrap_or("default.conf");
    let p2p = matches.value_of("p2p").unwrap_or("127.0.0.1:8846");
    let rpc = matches.value_of("rpc").unwrap_or("127.0.0.1:8845");
    let peers = matches.value_of("peers").unwrap_or("").split(",").map(|s| s.to_string()).collect();
    Conf {config: config.to_string(), p2p: p2p.to_string(), rpc: rpc.to_string(), peers: peers}
}

fn start_p2p(conf: Conf) {
    if conf.p2p == "none" {
        return
    }
}