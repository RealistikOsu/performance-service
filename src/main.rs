use clap::Parser;
use deadpool_lapin::{Manager, Pool};
use lapin::ConnectionProperties;
use performance_service::{api, config::Config, context::Context, deploy, mass_recalc};
use redis::Client;
use sqlx::mysql::MySqlPoolOptions;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenv::dotenv().ok();
    env_logger::init();

    let config = Config::parse();

    let mysql_url = format!(
        "mysql://{}:{}@{}:{}/{}",
        config.mysql_user,
        config.mysql_password,
        config.mysql_host,
        config.mysql_port,
        config.mysql_database,
    );

    let database = MySqlPoolOptions::new()
        .connect(&mysql_url)
        .await?;

    let redis_url = format!(
        "redis://{}:{}@{}:{}/{}",
        config.redis_user,
        config.redis_password,
        config.redis_host,
        config.redis_port,
        config.redis_db,
    );

    let redis = Client::open(redis_url)?;

    let context = Context {
        config,
        database,
        redis,
    };

    match context.config.app_component.as_str() {
        "api" => api::serve(context).await?,
        "mass_recalc" => mass_recalc::serve(context).await?,
        "deploy" => deploy::serve(context).await?,
        "recalc" => deploy::recalc_single(context).await?,
        _ => panic!("unknown app component"),
    }

    Ok(())
}
