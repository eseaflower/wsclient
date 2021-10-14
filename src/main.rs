use anyhow::Result;
use structopt::StructOpt;

#[derive(StructOpt, Debug)]
struct Opt {
    /// The addres of the web-socket server.
    #[structopt(long, short, default_value = "ws://localhost:7979")]
    ws_url: String,
    #[structopt(long, short)]
    case: Option<String>,
    #[structopt(long, short)]
    protocol: Option<String>,
    #[structopt(long, default_value = "640")]
    width: u32,
    #[structopt(long, default_value = "480")]
    height: u32,
    #[structopt(long, default_value = "1.0")]
    bitrate_scale: f32,
    #[structopt(long)]
    cpu: bool,
    #[structopt(long, default_value = "default")]
    preset: String,
    #[structopt(long)]
    lossless: bool,
    #[structopt(long, default_value = "1.0")]
    video_scaling: f32,
    #[structopt(long)]
    narrow: bool,
    #[structopt(long)]
    tcp: bool,
    #[structopt(long)]
    client_hw: bool,
    #[structopt(long)]
    fast_sw: bool,
    #[structopt(long, default_value = "200")]
    jitter: u32,
    #[structopt(long, default_value = "1")]
    views: usize,
    #[structopt(long, default_value = "default")]
    rate_schedule: String,
}

fn main() -> Result<()> {
    env_logger::init();
    let opt = Opt::from_args();

    let config = wsclient::AppConfig::new(
        opt.ws_url,
        (opt.width, opt.height),
        opt.case,
        opt.protocol,
        opt.bitrate_scale,
        !opt.cpu,
        opt.preset,
        opt.lossless,
        opt.video_scaling,
        opt.narrow,
        opt.tcp,
        opt.client_hw,
        opt.fast_sw,
        opt.jitter,
        opt.views,
        opt.rate_schedule,
    );
    log::info!("Running with config: {:?}", &config);
    wsclient::run(config)
}
