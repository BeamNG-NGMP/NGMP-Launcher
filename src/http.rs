use warp::Filter;
use serde::{Serialize, Deserialize};
use tokio::sync::{Mutex, mpsc};
use std::sync::Arc;
use url::Url;

use crate::signal::SignalReceiver;

// const LOGIN_API: &'static str = "http://127.0.0.1:11281";
// const LOGIN_API: &'static str = "http://login.ngmp.net:11281";
const LOGIN_API: &'static str = "http://138.201.33.234:11281";

pub async fn request_user_data(auth_token: String) -> anyhow::Result<UserAuth> {
    let url = format!("{LOGIN_API}/login_auth/{}", auth_token);

    let client = reqwest::Client::new();
    Ok(client.get(url)
        .send()
        .await?
        .json()
        .await?)
}

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

fn make_err(e: String) -> warp::http::Response<Vec<u8>> {
    warp::http::Response::builder()
        .body(e.as_bytes().to_vec()).unwrap()
}

async fn get_resp(url: String) -> warp::http::Response<Vec<u8>> {
    let client = reqwest::Client::new();
    match client.get(url).send().await {
        Err(e) => make_err(format!("<h1>{}</h1>", e)),
        Ok(resp) => {
            let mut resp_builder = warp::http::Response::builder();
            for (k, v) in resp.headers().iter() {
                resp_builder = resp_builder.header(k.as_str(), v.as_bytes());
            }
            resp_builder = resp_builder.status(resp.status().as_u16());
            match resp.bytes().await {
                Err(e) => make_err(format!("<h1>{}</h1>", e)),
                Ok(bytes) => {
                    match resp_builder.body(bytes.to_vec()) {
                        Err(e) => make_err(format!("<h1>{}</h1>", e)),
                        Ok(r) => r,
                    }
                },
            }
        },
    }
}

async fn http_async(http_port: u16, message_tx: mpsc::Sender<Message>) {
    let mut latest_address = Arc::new(Mutex::new(String::new()));

    let index = {
        let latest_address = latest_address.clone();
        warp::any().and(warp::path::full()).and_then(move |full_url: warp::path::FullPath| {
            let addr = latest_address.clone();
            async move {
                let mut url = {
                    let base = addr.lock().await;
                    format!("{base}{}", full_url.as_str())
                };
                if url.starts_with("/") {
                    url = url[1..].to_string();
                }
                error!("url: {}", url);

                Ok::<_, warp::Rejection>(get_resp(url).await)
            }
        })
    };

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

    let external = {
        let latest_address = latest_address.clone();
        warp::path::path("external").and(warp::path::full()).and_then(move |full_url: warp::path::FullPath| {
            let addr = latest_address.clone();
            async move {
                let url = full_url.as_str()[10..].to_string();
                error!("URL: {}", url);

                let parsed_url = match Url::parse(&url) {
                    Ok(url) => url,
                    Err(e) => {
                        return Ok::<_, warp::Rejection>(make_err(format!("<h1>{}</h1>", e)));
                    }
                };

                {
                    let mut addr = addr.lock().await;
                    let origin = parsed_url.origin().ascii_serialization();
                    let new_base = format!("{origin}");
                    error!("NEW BASEEEEEEEEE: {}", new_base);
                    *addr = new_base;
                }

                Ok::<_, warp::Rejection>(get_resp(url).await)
            }
        })
    };

    let server_list = warp::path("server_list").map(|| {
        let server_list = get_server_list();
        warp::reply::json(&server_list)
    });

    let routes = warp::get().and(
        login
            .or(login_callback)
            .or(external)
            // This one must always be last
            .or(index)
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
