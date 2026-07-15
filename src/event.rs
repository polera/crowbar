use std::time::Duration;

use crossterm::event::EventStream;
use futures::StreamExt;
use tokio::sync::mpsc;

use crate::channel::ProxyToUi;

#[derive(Debug)]
pub enum AppEvent {
    Input(crossterm::event::Event),
    Proxy(ProxyToUi),
    Tick,
}

pub struct EventLoop {
    rx: mpsc::Receiver<AppEvent>,
}

impl EventLoop {
    pub fn new(mut proxy_rx: mpsc::Receiver<ProxyToUi>) -> Self {
        let (tx, rx) = mpsc::channel(1_024);

        let input_tx = tx.clone();
        tokio::spawn(async move {
            let mut stream = EventStream::new();
            while let Some(Ok(event)) = stream.next().await {
                if input_tx.send(AppEvent::Input(event)).await.is_err() {
                    break;
                }
            }
        });

        let proxy_tx = tx.clone();
        tokio::spawn(async move {
            while let Some(msg) = proxy_rx.recv().await {
                if proxy_tx.send(AppEvent::Proxy(msg)).await.is_err() {
                    break;
                }
            }
        });

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(250));
            loop {
                interval.tick().await;
                if tx.send(AppEvent::Tick).await.is_err() {
                    break;
                }
            }
        });

        Self { rx }
    }

    pub async fn next(&mut self) -> Option<AppEvent> {
        self.rx.recv().await
    }
}
