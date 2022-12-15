use crate::{
    context::Context,
    models::leaderboard::Leaderboard,
    models::stats::APIReworkStats,
    models::{rework::Rework, stats::APIBaseReworkStats},
};
use std::sync::Arc;

pub struct LeaderboardsRepository {
    context: Arc<Context>,
}

impl LeaderboardsRepository {
    pub fn new(context: Arc<Context>) -> Self {
        Self { context }
    }

    pub async fn fetch_one(
        &self,
        rework_id: i32,
        offset: i32,
        limit: i32,
    ) -> anyhow::Result<Option<Leaderboard>> {
        let rework: Rework = match sqlx::query_as(r#"SELECT * FROM reworks WHERE rework_id = ?"#)
            .bind(rework_id)
            .fetch_optional(&self.context.database)
            .await?
        {
            Some(rework) => rework,
            None => return Ok(None),
        };

        let leaderboard_count: i32 =
            sqlx::query_scalar("SELECT COUNT(*) FROM rework_stats WHERE rework_id = ?")
                .bind(rework.rework_id)
                .fetch_one(&self.context.database)
                .await
                .unwrap();

        let rework_users: Vec<APIBaseReworkStats> = sqlx::query_as(
            "SELECT user_id, users_stats.country, users.username user_name, rework_id, old_pp, new_pp 
            FROM 
                rework_stats 
            INNER JOIN 
                users_stats
                ON users_stats.id = rework_stats.user_id
            INNER JOIN
                users
                ON users.id = rework_stats.user_id
            WHERE 
                rework_id = ?
            ORDER BY 
                new_pp DESC
            LIMIT ?, ?"
        )
            .bind(rework.rework_id)
            .bind(offset)
            .bind(limit)
            .fetch_all(&self.context.database)
            .await
            .unwrap();

        let mut rework_stats = Vec::new();
        for rework_user in &rework_users {
            let mut temp_users = rework_users.clone();

            temp_users.sort_by(|a, b| a.old_pp.partial_cmp(&b.old_pp).unwrap());
            temp_users.reverse();
            let old_rank = (temp_users
                .iter()
                .position(|a| a.user_id == rework_user.user_id)
                .unwrap()
                + 1) as u64
                + offset as u64;

            temp_users.sort_by(|a, b| a.new_pp.partial_cmp(&b.new_pp).unwrap());
            temp_users.reverse();
            let new_rank = (temp_users
                .iter()
                .position(|a| a.user_id == rework_user.user_id)
                .unwrap()
                + 1) as u64
                + offset as u64;

            let rework_user = APIReworkStats::from_base(rework_user.clone(), new_rank, old_rank);
            rework_stats.push(rework_user);
        }

        let leaderboard = Leaderboard {
            total_count: leaderboard_count,
            users: rework_stats,
        };

        Ok(Some(leaderboard))
    }
}
