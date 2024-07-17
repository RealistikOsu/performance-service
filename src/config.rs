#[derive(clap::Parser, Clone)]
pub struct Config {
    #[clap(long, env)]
    pub app_component: String,

    #[clap(long, env)]
    pub api_host: String,

    #[clap(long, env)]
    pub api_port: Option<u16>,

    #[clap(long, env)]
    pub mysql_user: String,

    #[clap(long, env)]
    pub mysql_password: String,

    #[clap(long, env)]
    pub mysql_host: String,

    #[clap(long, env)]
    pub mysql_port: u16,

    #[clap(long, env)]
    pub mysql_database: String,

    #[clap(long, env)]
    pub amqp_user: String,

    #[clap(long, env)]
    pub amqp_password: String,

    #[clap(long, env)]
    pub amqp_host: String,

    #[clap(long, env)]
    pub amqp_port: u16,

    #[clap(long, env)]
    pub redis_user: String,

    #[clap(long, env)]
    pub redis_password: String,

    #[clap(long, env)]
    pub redis_db: u8,

    #[clap(long, env)]
    pub redis_host: String,

    #[clap(long, env)]
    pub redis_port: u16,

    #[clap(long, env)]
    pub beatmaps_path: String,
}
