use std::{env, error::Error, sync::Arc, time::Duration};
use std::net::SocketAddr;

use axum::{Extension, Json, Router};
use axum::extract::Path;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect};
use axum::routing::{get, post};
//use axum_sqlx_tx::{Layer, Tx};
use dotenv::dotenv;
use reqwest::Url;
use sqlx::{PgPool, Pool, Postgres, postgres::PgPoolOptions, query, query_as};
use teloxide::{dispatching::update_listeners::webhooks, error_handlers::IgnoringErrorHandlerSafe,
               prelude::*, types::{InlineKeyboardButton, InlineKeyboardMarkup, Update}};
use teloxide::adaptors::DefaultParseMode;
use teloxide::types::ParseMode;

use crate::models::{ApiError, InputReport, Report};

mod models;

const REPORT_GROUP:ChatId = ChatId(-1001758396624);

#[tokio::main]
async fn main() {
    dotenv().ok();
    pretty_env_logger::init();
    log::info!("Starting tartaros-telegram...");

    let bot: AutoSend<DefaultParseMode<Bot>> = Bot::from_env().parse_mode(ParseMode::Html).auto_send();

    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&*env::var("DATABASE_URL").expect("DATABASE_URL must be provided!")).await.unwrap();

    let handler = dptree::entry()
        .branch(Update::filter_callback_query().endpoint(callback_handler));
    //TODO: filter on report_group

    let b = bot.clone();
    let p = pool.clone();

    let token = bot.inner().inner().token();

    // Heroku auto defines a port value
    let port: u16 = env::var("PORT")
        .expect("PORT env variable is not set")
        .parse()
        .expect("PORT env variable value is not an integer");

    let addr = SocketAddr::from(([0, 0, 0, 0], port));

    let host = env::var("HOST").expect("HOST env variable is not set");
    let url = Url::parse(&format!("https://{host}/webhooks/{token}")).unwrap();

    let listener = webhooks::axum_to_router(bot.clone(), webhooks::Options::new(*&addr, url))
        .await
        .expect("Couldn't setup webhook");

    let app = listener.2
        .route("/", get(redirect_readme))
        .route("/reports", post(report_user))
        .route("/users/:user_id", get(user_by_id))
        .route("/users", post(users_by_id))
        .layer(Extension(b))
        .layer(Extension(p));

    let sr = axum::Server::bind(&addr)
        .serve(app.into_make_service());


    let mut dp = Dispatcher::builder(bot, handler)
        .dependencies(dptree::deps![pool])
   //     .enable_ctrlc_handler()
        .build();
    let d = dp
        .dispatch_with_listener(listener.0, Arc::new(IgnoringErrorHandlerSafe));



    let (_, _) = tokio::join!(sr, d);
}


async fn callback_handler(
    q: CallbackQuery,
    bot: AutoSend<DefaultParseMode<Bot>>,
    pool: Pool<Postgres>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    println!("click");

    if let Some(report) = q.data {
        if let Some(Message { id, chat, .. }) = q.message {
            let ban: char = report.chars().next().unwrap();
            let report_id: i32 = report[1..report.len()].parse::<i32>().unwrap();

            println!("ban: {} - id: {}", ban, report_id);

            if ban == 'y' {
                let _result = query!(r#"update reports set is_banned = true where id = $1"#,report_id)
                    .execute(&pool).await?;
            }

            //maybe edit text and append "reported" or "declined" ?
            bot.edit_message_reply_markup(chat.id, id).await?;
            bot.edit_message_text(chat.id, id, "WEBHOOK WORKS yeu2y".to_owned()).await?;

            println!("test");
        }
    }

    Ok(()) //     respond(())
}

async fn send_report(
    bot: &AutoSend<DefaultParseMode<Bot>>,
    pool: &Pool<Postgres>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let reports: Vec<Report> = query_as!(Report,
        r#"select * from reports where is_banned IS NULL"#)
        .fetch_all(pool)
        .await?;

    println!("Report length {}", reports.len());

    if !reports.is_empty() {
        for report in reports {
            println!("{}", &report.id);

            let keyboard = InlineKeyboardMarkup::new(vec![
                vec![
                    InlineKeyboardButton::callback("Ban user ??????", format!("y{}", &report.id))
                ],
                vec![
                    InlineKeyboardButton::callback("Cancel report ????", format!("n{}", &report.id))
                ]]);

            //TODO: .parse_mode(ParseMode::Html) and format user_id to link to user
            bot.send_message(REPORT_GROUP,
                             format!("???? {} - ?????? {} - ???? {}\n\n{}",
                                     &report.id, &report.account_id, &report.user_id, &report.message))
                .reply_markup(keyboard).await.expect("Failed to send message");

            query!(r#"update reports set is_banned = false where id = $1"#,&report.id)
                .execute(pool).await?;
        }
    }

    Ok(())
}

async fn redirect_readme() -> Redirect {
    Redirect::to("https://github.com/PXNX/tartaros-telegram#readme")
}


async fn users_by_id(
    Extension(pool): Extension<Pool<Postgres>>,
    user_ids : Json<Vec<i64>>,
) -> Result<Json<Report>, (StatusCode, Json<ApiError>)> {
    query_as!(Report, r#"Select * from reports where user_id in $1 and is_banned=true"#, user_ids).fetch_all(&pool)
        .await
        .map(Json)
        .map_err(|e|
            (StatusCode::NOT_FOUND, Json(ApiError {
                details: e.to_string(),
            }))
        )
}

async fn user_by_id(
    Extension(pool): Extension<Pool<Postgres>>,
    Path(user_id): Path<i64>,
) -> Result<Json<Report>, (StatusCode, Json<ApiError>)> {
    query_as!(Report, r#"Select * from reports where user_id = $1 and is_banned=true"#, user_id).fetch_one(&pool)
        .await
        .map(Json)
        .map_err(|e|
            (StatusCode::NOT_FOUND, Json(ApiError {
                details: e.to_string(),
            }))
        )
}

async fn report_user(
    Extension(pool): Extension<Pool<Postgres>>,
    Extension(bot): Extension<AutoSend<DefaultParseMode<Bot>>>,
    report: Json<InputReport>,
) -> Result<(StatusCode, Json<Report>), Json<ApiError>> {
    let result = sqlx::query_as!(Report, r#"Insert into reports (user_id, account_id, message) values ($1, $2, $3) returning *"#, report.user_id, 1, report.message).fetch_one(&pool)
        .await;

    return match result {
        Ok(res) => {
            send_report(&bot, &pool).await.unwrap();

            Ok((StatusCode::CREATED, Json(res)))
        }
        Err(e) => Err(Json(ApiError {
            details: e.to_string()
        }))
    };
}