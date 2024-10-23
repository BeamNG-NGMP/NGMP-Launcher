use warp::Filter;
use serde::{Serialize, Deserialize};

use crate::signal::SignalReceiver;

pub fn http_main(kill_signal: SignalReceiver, http_port: u16) {
    info!("Http server started!");

    let rt = tokio::runtime::Runtime::new().expect("Failed to create runtime for HTTP server!");
    let handle = rt.spawn(async move { http_async(http_port).await; });

    loop {
        if !kill_signal.alive() {
            break;
        }

        std::thread::sleep(std::time::Duration::from_secs(1));
    }

    handle.abort();

    info!("Http server stopped!");
}

async fn http_async(http_port: u16) {
    let index = warp::path::end().map(|| "hi how'd you get here!!!");

    let server_list = warp::path("server_list").map(|| {
        let server_list = get_server_list();
        warp::reply::json(&server_list)
    });

    let routes = warp::get().and(
        index
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
