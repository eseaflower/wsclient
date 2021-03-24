use anyhow::Result;
use structopt::StructOpt;
#[derive(StructOpt, Debug)]
struct Opt {
    /// The addres of the web-socket server.
    #[structopt(long, short, default_value = "ws://localhost:7979")]
    ws_url: String,
}

fn main() -> Result<()> {
    env_logger::init();
    let opt = Opt::from_args();

    log::info!("Testing info");
    // async_std::task::block_on(wsclient::run(opt.ws_url))
    wsclient::run(opt.ws_url)
}
