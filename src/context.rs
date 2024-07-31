use redis::Client;
use sqlx::MySqlPool;

use crate::config::Config;

#[derive(Clone)]
pub struct Context {
    pub config: Config,
    pub database: MySqlPool,
    pub redis: Client,
}
