use crate::context::Context;
use crate::models::queue::QueueResponse;
use crate::models::rework::Rework;
use crate::repositories;
use redis::AsyncCommands;
use std::sync::Arc;

#[derive(serde::Serialize, serde::Deserialize)]
pub struct CreateSessionResponse {
    pub success: bool,
    pub user_id: Option<i32>,
    pub session_token: Option<String>,
}

pub async fn create(
    username: String,
    password_md5: String,
    context: Arc<Context>,
) -> CreateSessionResponse {
    let user_info: Option<(i32, String)> =
        sqlx::query_as("SELECT id, password_md5 FROM users WHERE username_safe = ?")
            .bind(&username.to_lowercase().replace(" ", "_"))
            .fetch_optional(&context.database)
            .await
            .unwrap();

    if user_info.is_none() {
        return CreateSessionResponse {
            success: false,
            user_id: None,
            session_token: None,
        };
    }

    let (user_id, database_bcrypt) = user_info.unwrap();

    let correct_password = bcrypt::verify(&password_md5, &database_bcrypt).unwrap_or(false);
    if !correct_password {
        return CreateSessionResponse {
            success: false,
            user_id: None,
            session_token: None,
        };
    }

    let repo = repositories::sessions::SessionsRepository::new(context);
    let session_token = repo.create(user_id).await.unwrap();

    CreateSessionResponse {
        success: true,
        user_id: Some(user_id),
        session_token: Some(session_token),
    }
}

pub async fn delete(session_token: String, context: Arc<Context>) -> anyhow::Result<()> {
    let repo = repositories::sessions::SessionsRepository::new(context);
    repo.delete(session_token).await?;

    Ok(())
}

pub async fn enqueue(
    session_token: String,
    rework_id: i32,
    context: Arc<Context>,
) -> anyhow::Result<QueueResponse> {
    let mut redis_conn = context.redis.get_async_connection().await.unwrap();
    let user_id: i32 = redis_conn
        .get(format!("rework:sessions:{}", session_token))
        .await
        .unwrap_or(0);

    if user_id == 0 {
        return Ok(QueueResponse {
            success: false,
            message: Some("Invalid session token".to_string()),
        });
    }

    let user_privileges: Option<(i32,)> =
        sqlx::query_as(r#"SELECT privileges FROM users WHERE id = ?"#)
            .bind(user_id)
            .fetch_optional(&context.database)
            .await
            .unwrap();

    if user_privileges.is_none() {
        return Ok(QueueResponse {
            success: false,
            message: Some("User does not exist".to_string()),
        });
    }

    if user_privileges.unwrap().0 & 1 == 0 {
        return Ok(QueueResponse {
            success: false,
            message: Some("User is restricted".to_string()),
        });
    }

    let rework: Rework = sqlx::query_as(r#"SELECT * FROM reworks WHERE rework_id = ?"#)
        .bind(rework_id)
        .fetch_one(&context.database)
        .await
        .unwrap();

    let in_queue: Option<(i32,)> = sqlx::query_as(
        r#"SELECT 1 FROM rework_queue WHERE user_id = ? AND rework_id = ? AND processed_at < ?"#,
    )
    .bind(user_id)
    .bind(rework_id)
    .bind(rework.updated_at)
    .fetch_optional(&context.database)
    .await
    .unwrap();

    if in_queue.is_some() {
        return Ok(QueueResponse {
            success: false,
            message: Some("Already in queue".to_string()),
        });
    }

    sqlx::query(r#"REPLACE INTO rework_queue (user_id, rework_id) VALUES (?, ?)"#)
        .bind(user_id)
        .bind(rework_id)
        .execute(&context.database)
        .await
        .unwrap();

    Ok(QueueResponse {
        success: true,
        message: None,
    })
}
