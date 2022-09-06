use std::collections::{HashMap, HashSet};

use std::str::FromStr;

use serde::Deserialize;
use serde_aux::prelude::*;
use serde_json::Value;

use sqlx::sqlite::SqlitePool;

use tokio::time::{sleep, Duration};

use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Deserialize, Debug)]
struct Bet {
    #[serde(rename = "n")]
    name: String,
    b: String,
    #[serde(deserialize_with = "deserialize_number_from_string")]
    p: u32,
    #[serde(deserialize_with = "deserialize_number_from_string")]
    w: i64,
    r: String,
    g: String,
    c: String,
}

#[derive(Deserialize, Debug)]
struct ZData {
    p1name: String,
    p1total: String,
    p2name: String,
    p2total: String,
    status: String,
    alert: String,
    x: i64,
    remaining: String,

    #[serde(flatten)]
    bets: Option<HashMap<String, Bet>>,
}

async fn read_zdata_status() -> Result<ZData, Box<dyn std::error::Error>> {
    let resp = reqwest::get("https://www.saltybet.com/state.json")
        .await?
        .bytes()
        .await?;

    Ok(serde_json::from_slice(&resp)?)
}

async fn read_zdata() -> Result<ZData, Box<dyn std::error::Error>> {
    let resp = reqwest::get("https://www.saltybet.com/zdata.json")
        .await?
        .bytes()
        .await?;

    Ok(serde_json::from_slice(&resp)?)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let pool = SqlitePool::connect("test.db").await?;

    sqlx::query(
        r#"
CREATE TABLE IF NOT EXISTS fighters (
  name text not null primary key
);"#,
    )
    .execute(&pool)
    .await?;

    sqlx::query(
        r#"
CREATE TABLE IF NOT EXISTS bettors (
  id integer not null primary key,
  name text non null
);"#,
    )
    .execute(&pool)
    .await?;

    sqlx::query(
        r#"
CREATE TABLE IF NOT EXISTS matches (
  id integer not null primary key,
  time integer non null,
  fighter1 text non null,
  fighter2 text non null,
  winner text non null,
  foreign key (fighter1) references fighters (name),
  foreign key (fighter2) references fighters (name),
  foreign key (winner) references fighters (name)
);"#,
    )
    .execute(&pool)
    .await?;

    sqlx::query(
        r#"
CREATE TABLE IF NOT EXISTS bets (
  match integer non null,
  amount integer non null,
  bettor integer non null,
  fighter text non null,
  foreign key (bettor) references bettors (id),
  foreign key (match) references matches (id),
  foreign key (fighter) references fighters (name)
);"#,
    )
    .execute(&pool)
    .await?;

    'match_loop: loop {
        println!("starting sequence!");

        while read_zdata_status().await?.status != "locked" {
            println!("fresh match hasn't started yet");
            sleep(Duration::from_millis(500)).await;
        }

        println!("Match locked in, awaiting results.");

        let betting_data: ZData = read_zdata().await?;

        if betting_data.status != "locked" {
            println!("status changed before we could get to it!");
            continue 'match_loop;
        }

        let match_timestamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();

        println!("{} bets", betting_data.bets.as_ref().unwrap().len());

        let winning_team = loop {
            let status = read_zdata_status().await?.status;
            if let Ok(winning_team) = status.parse::<u64>() {
                break winning_team;
            }

            if betting_data.status != "locked" {
                println!("status changed before we could get to it!");
                continue 'match_loop;
            }

            sleep(Duration::from_millis(500)).await;
        };

        println!("team {} has won this round.", winning_team);

        let bets = betting_data.bets.unwrap();

        let (team1, team2) = bets.values().fold((0i64, 0i64), |(a, b), v| {
            if v.p == 1 {
                (a + v.w, b)
            } else {
                (a, b + v.w)
            }
        });

        println!("team1: {}$, team2: {}$", team1, team2);

        sqlx::query("insert or ignore into fighters (name) values ($1)")
            .bind(&betting_data.p1name)
            .execute(&pool)
            .await?;

        sqlx::query("insert or ignore into fighters (name) values ($1)")
            .bind(&betting_data.p2name)
            .execute(&pool)
            .await?;

        let (match_id,): (i64,) = sqlx::query_as(
            "insert into matches (time, fighter1, fighter2, winner) values ($1, $2, $3, $4) returning id",
        )
        .bind(match_timestamp.as_secs() as u32)
        .bind(&betting_data.p1name)
        .bind(&betting_data.p2name)
        .bind(if winning_team == 1 {
            &betting_data.p1name
        } else {
            &betting_data.p2name
        })
        .fetch_one(&pool)
        .await?;

        for (k, v) in &bets {
            let bettor_id = k.parse::<u32>().unwrap();
            sqlx::query("insert or ignore into bettors (id, name) values ($1, $2)")
                .bind(bettor_id)
                .bind(&v.name)
                .execute(&pool)
                .await?;

            sqlx::query(
                "insert into bets (match, amount, bettor, fighter) values ($1, $2, $3, $4)",
            )
            .bind(match_id)
            .bind(v.w)
            .bind(bettor_id)
            .bind(if v.p == 1 {
                &betting_data.p1name
            } else {
                &betting_data.p2name
            })
            .execute(&pool)
            .await?;
        }
    }

    Ok(())
}
