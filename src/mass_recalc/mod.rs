use std::io::Write;
use std::sync::Arc;
use std::time::SystemTime;

use crate::{
    context::Context,
    models::{queue::QueueRequest, rework::Rework},
    usecases,
};

use lapin::{options::BasicPublishOptions, BasicProperties};
use redis::AsyncCommands;

async fn queue_user(user_id: i32, rework: &Rework, context: &Context) {
    let in_queue: Option<bool> = sqlx::query_scalar(
        "SELECT 1 FROM rework_queue WHERE user_id = ? AND rework_id = ? AND processed_at < ?",
    )
    .bind(user_id)
    .bind(rework.rework_id)
    .bind(rework.updated_at)
    .fetch_optional(&context.database)
    .await
    .unwrap();

    if in_queue.is_some() {
        return;
    }

    sqlx::query(r#"REPLACE INTO rework_queue (user_id, rework_id) VALUES (?, ?)"#)
        .bind(user_id)
        .bind(rework.rework_id)
        .execute(&context.database)
        .await
        .unwrap();

    context
        .amqp_channel
        .basic_publish(
            "",
            "rework_queue",
            BasicPublishOptions::default(),
            &rkyv::to_bytes::<_, 256>(&QueueRequest {
                user_id,
                rework_id: rework.rework_id,
            })
            .unwrap(),
            BasicProperties::default(),
        )
        .await
        .unwrap();

    log::info!("Queued user ID {}", user_id);
}

pub async fn serve(context: Context) -> anyhow::Result<()> {
    print!("Enter a rework ID to mass recalculate: ");
    std::io::stdout().flush().unwrap();

    let mut rework_id_str = String::new();
    std::io::stdin().read_line(&mut rework_id_str)?;
    let rework_id = rework_id_str.trim().parse::<i32>()?;

    print!("\n");
    std::io::stdout().flush().unwrap();

    log::info!("Mass recalculating on rework ID {}", rework_id);

    let rework = usecases::reworks::fetch_one(rework_id, Arc::from(context.clone()))
        .await?
        .unwrap();

    sqlx::query("DELETE FROM rework_scores WHERE rework_id = ?")
        .bind(rework_id)
        .execute(&context.database)
        .await?;

    sqlx::query("DELETE FROM rework_stats WHERE rework_id = ?")
        .bind(rework_id)
        .execute(&context.database)
        .await?;

    sqlx::query("DELETE FROM rework_queue WHERE rework_id = ?")
        .bind(rework_id)
        .execute(&context.database)
        .await?;

    let mut redis_connection = context.redis.get_async_connection().await?;
    let _: () = redis_connection
        .del(format!("rework:leaderboard:{}", rework_id))
        .await?;

    let stats_prefix = match rework.mode {
        0 => "std",
        1 => "taiko",
        2 => "ctb",
        3 => "mania",
        _ => unreachable!(),
    };

    let stats_table = match rework.rx {
        0 => "users_stats",
        1 => "rx_stats",
        2 => "ap_stats",
        _ => unreachable!(),
    };

    let user_ids: Vec<(i32,)> = sqlx::query_as(&format!("SELECT users.id, pp_{} pp FROM {} INNER JOIN users USING(id) WHERE pp_{} > 0 AND users.privileges & 1 ORDER BY pp desc", stats_prefix, stats_table, stats_prefix))
        .fetch_all(&context.database)
        .await?;

    for (user_id,) in user_ids {
        queue_user(user_id, &rework, &context).await;
    }

    Ok(())
}
