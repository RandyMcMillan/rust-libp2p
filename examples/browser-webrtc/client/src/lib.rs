mod pinger;

use futures::channel;
use futures::StreamExt;
use js_sys::Date;
use pinger::start_pinger;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::spawn_local;

#[wasm_bindgen(start)]
pub async fn run() -> Result<(), JsValue> {
    wasm_logger::init(wasm_logger::Config::default());

    // Use `web_sys`'s global `window` function to get a handle on the global
    // window object.
    let window = web_sys::window().expect("no global `window` exists");
    let document = window.document().expect("should have a document on window");
    let body = document.body().expect("document should have a body");

    // Manufacture the element we're gonna append
    let val = document.create_element("p")?;
    val.set_text_content(Some("Let's ping the WebRTC Server!"));

    body.append_child(&val)?;

    // create a mpsc channel to get pings to from the pinger
    let (sendr, mut recvr) = channel::mpsc::channel::<Result<f32, pinger::Error>>(2);

    // start the pinger, pass in our sender
    spawn_local(async move {
        match start_pinger(sendr).await {
            Ok(_) => log::info!("Pinger finished"),
            Err(e) => log::info!("Pinger error: {:?}", e),
        };
    });

    // loop on recvr await, appending to the DOM with date and RTT when we get it
    loop {
        match recvr.next().await {
            Some(Ok(rtt)) => {
                log::info!("Got RTT: {}", rtt);
                let val = document
                    .create_element("p")
                    .expect("should create a p elem");
                val.set_text_content(Some(&format!(
                    "RTT: {}ms at {}",
                    rtt,
                    Date::new_0().to_string()
                )));
                body.append_child(&val).expect("should append body elem");
            }
            Some(Err(e)) => log::info!("Error: {:?}", e),
            None => log::info!("Recvr channel closed"),
        }
    }
}