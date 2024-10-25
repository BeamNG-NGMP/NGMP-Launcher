use warp::Filter;
use serde::{Serialize, Deserialize};
use tokio::sync::mpsc;

use crate::signal::SignalReceiver;

// const LOGIN_API: &'static str = "http://127.0.0.1:11281";
// const LOGIN_API: &'static str = "http://login.ngmp.net:11281";
const LOGIN_API: &'static str = "http://138.201.33.234:11281";

#[derive(Debug)]
pub enum Message {
    AuthFailure,
    AuthData(UserAuth),
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UserAuth {
    pub auth: String,
    pub steam_id: u64,
    pub user: User,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct User {
    pub name: String,
    pub avatar_hash: String,
}

pub fn http_main(kill_signal: SignalReceiver, http_port: u16, message_tx: mpsc::Sender<Message>) {
    info!("Http server started!");

    let rt = tokio::runtime::Runtime::new().expect("Failed to create runtime for HTTP server!");
    let handle = rt.spawn(async move { http_async(http_port, message_tx).await; });

    loop {
        if !kill_signal.alive() {
            break;
        }

        std::thread::sleep(std::time::Duration::from_secs(1));
    }

    handle.abort();

    info!("Http server stopped!");
}

async fn http_async(http_port: u16, message_tx: mpsc::Sender<Message>) {
    let index = warp::path::end().and_then(|| async move { Ok::<_, warp::Rejection>("hi how'd you get here!!!") });

    let login = warp::path("login").and_then(|| async move {
        let redirector = steam_auth::Redirector::new("http://localhost:4434", "/login_callback").unwrap();

        Ok::<_, warp::Rejection>(warp::reply::with_header(
            warp::reply::with_status("", warp::http::StatusCode::FOUND),
            "Location",
            redirector.url().as_str()
        ))
    });

    let login_callback = warp::path("login_callback").and(warp::query::raw()).and_then(move |q| {
        let tx = message_tx.clone();
        async move {
            let endpoint = format!("{LOGIN_API}/login?{}", q);

            let client = reqwest::Client::new();
            match client.get(endpoint)
                .send()
                .await
            {
                Err(e) => {
                    error!("{}", e);
                    tx.send(Message::AuthFailure).await;
                    Ok::<_, warp::Rejection>("Error occured :(")
                },
                Ok(res) => {
                    match res.json::<UserAuth>().await {
                        Err(e) => {
                            error!("{}", e);
                            tx.send(Message::AuthFailure).await;
                            Ok::<_, warp::Rejection>("Error occured :(")
                        },
                        Ok(user_auth) => {
                            tx.send(Message::AuthData(user_auth)).await;
                            Ok::<_, warp::Rejection>("Success! You can now close this page.")
                        }
                    }
                },
            }
        }
    });

    let server_list = warp::path("server_list").map(|| {
        let server_list = get_server_list();
        warp::reply::json(&server_list)
    });

    let routes = warp::get().and(
        index
            .or(login)
            .or(login_callback)
    );

    warp::serve(routes)
        .run(([127,0,0,1], http_port))
        .await;
}

#[derive(Serialize, Deserialize)]
pub struct ServerList {

}

fn get_server_list() -> ServerList {
    // TODO: Implement actual server list here
    ServerList {

    }
}
