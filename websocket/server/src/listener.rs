use std::{
    net::SocketAddr,
    sync::{
        Arc, Condvar, Mutex, Weak,
        atomic::{AtomicUsize, Ordering},
    },
    time::UNIX_EPOCH,
};

use tokio::{
    net::{TcpListener, TcpStream},
    sync::{mpsc, oneshot},
};
use tokio_tungstenite::tungstenite::protocol::{CloseFrame, frame::coding::CloseCode};
use tracing::{info, warn};

use rsnano_node::{
    consensus::election::{ConfirmedElection, VoteSummary},
    handles::LedgerQueryHandle,
    WalletServices,
};
use rsnano_types::{Account, Amount, BlockSideband, SavedBlock};
use rsnano_websocket_messages::{
    ConfirmationJsonOptions, ElectionInfo, JsonSideband, JsonVoteSummary, MessageEnvelope, Topic,
};

use super::{ConfirmationOptions, Options, WebsocketSessionEntry};
use crate::{WebsocketSession, confirmation_message_factory::ConfirmationMessageFactory};

pub struct WebsocketListener {
    endpoint: Mutex<SocketAddr>,
    tx_stop: Mutex<Option<oneshot::Sender<()>>>,
    wallet_services: WalletServices,
    ledger: LedgerQueryHandle,
    topic_subscriber_count: Arc<[AtomicUsize; 11]>,
    sessions: Arc<Mutex<Vec<Weak<WebsocketSessionEntry>>>>,
    tokio: tokio::runtime::Handle,
    bound: Mutex<bool>,
    bound_condition: Condvar,
}

impl WebsocketListener {
    pub fn new(
        endpoint: SocketAddr,
        wallet_services: WalletServices,
        ledger: LedgerQueryHandle,
        tokio: tokio::runtime::Handle,
    ) -> Self {
        Self {
            endpoint: Mutex::new(endpoint),
            tx_stop: Mutex::new(None),
            wallet_services,
            ledger,
            topic_subscriber_count: Arc::new(std::array::from_fn(|_| AtomicUsize::new(0))),
            sessions: Arc::new(Mutex::new(Vec::new())),
            tokio,
            bound: Mutex::new(false),
            bound_condition: Condvar::new(),
        }
    }

    pub fn any_subscriber(&self, topic: Topic) -> bool {
        self.subscriber_count(topic) > 0
    }

    pub fn subscriber_count(&self, topic: Topic) -> usize {
        self.topic_subscriber_count[topic as usize].load(Ordering::SeqCst)
    }

    fn set_bound(&self) {
        *self.bound.lock().unwrap() = true;
        self.bound_condition.notify_one();
    }

    async fn run(&self) {
        let endpoint = *self.endpoint.lock().unwrap();
        let listener = match TcpListener::bind(endpoint).await {
            Ok(s) => s,
            Err(e) => {
                self.set_bound();
                warn!("Listen failed: {:?}", e);
                return;
            }
        };
        let ep = listener.local_addr().unwrap();
        *self.endpoint.lock().unwrap() = ep;
        self.set_bound();
        info!("Websocket listener started on {}", ep);

        let (tx_stop, rx_stop) = oneshot::channel::<()>();
        *self.tx_stop.lock().unwrap() = Some(tx_stop);

        tokio::select! {
            _ = rx_stop =>{},
           _ = self.accept(listener) =>{}
        }
    }

    /// Close all websocket sessions and stop listening for new connections
    pub async fn stop_async(&self) {
        let tx = self.tx_stop.lock().unwrap().take();
        if let Some(tx) = tx {
            tx.send(()).unwrap()
        }

        let mut sessions = self.sessions.lock().unwrap();
        for session in sessions.drain(..) {
            if let Some(session) = session.upgrade() {
                session.close();
            }
        }
    }

    pub fn listening_port(&self) -> u16 {
        self.endpoint.lock().unwrap().port()
    }

    /// Broadcast \p message to all session subscribing to the message topic.
    pub fn broadcast(&self, message: &MessageEnvelope) {
        let sessions = self.sessions.lock().unwrap();
        for session in sessions.iter() {
            if let Some(session) = session.upgrade() {
                let _ = session.blocking_write(message);
            }
        }
    }

    /// Broadcast block confirmation. The content of the message depends on subscription options (such as "include_block")
    pub fn broadcast_confirmation(
        &self,
        block: &SavedBlock,
        amount: &Amount,
        election: &ConfirmedElection,
    ) {
        if !self.any_subscriber(Topic::Confirmation) {
            return;
        }

        let sessions = self.sessions.lock().unwrap();
        for session in sessions.iter() {
            if let Some(session) = session.upgrade() {
                let subs = session.subscriptions.lock().unwrap();
                if let Some(options) = subs.get(&Topic::Confirmation) {
                    let default_opts = ConfirmationOptions::new(ConfirmationJsonOptions::default());
                    let conf_opts = if let Options::Confirmation(i) = options {
                        i
                    } else {
                        &default_opts
                    };

                    let message = ConfirmationMessageFactory {
                        ledger: &self.ledger,
                        options: conf_opts,
                        block,
                        amount,
                        election,
                    }
                    .create_message();

                    drop(subs);
                    let _ = session.blocking_write(&message);
                }
            }
        }
    }

    async fn accept(&self, listener: TcpListener) {
        loop {
            match listener.accept().await {
                Ok((stream, peer_addr)) => {
                    let wallet_services = self.wallet_services.clone();
                    let sub_count = Arc::clone(&self.topic_subscriber_count);
                    let (tx_send, rx_send) = mpsc::channel::<MessageEnvelope>(1024);
                    let sessions = Arc::clone(&self.sessions);
                    tokio::spawn(async move {
                        if let Err(e) = accept_connection(
                            stream,
                            wallet_services,
                            sub_count,
                            peer_addr,
                            tx_send,
                            rx_send,
                            sessions,
                        )
                        .await
                        {
                            warn!("listener failed: {:?}", e)
                        }
                    });
                }
                Err(e) => warn!("Accept failed: {:?}", e),
            }
        }
    }
}

pub trait WebsocketListenerExt {
    fn start(&self);
    fn stop(&self);
}

impl WebsocketListenerExt for Arc<WebsocketListener> {
    /// Start accepting connections
    fn start(&self) {
        let self_l = Arc::clone(self);
        self.tokio.spawn(async move {
            self_l.run().await;
        });
        let guard = self.bound.lock().unwrap();
        drop(self.bound_condition.wait_while(guard, |bound| !*bound));
    }

    fn stop(&self) {
        let self_l = Arc::clone(self);
        self.tokio.spawn(async move {
            self_l.stop_async().await;
        });
    }
}

async fn accept_connection(
    stream: TcpStream,
    wallet_services: WalletServices,
    topic_subscriber_count: Arc<[AtomicUsize; 11]>,
    peer_addr: SocketAddr,
    tx_send: mpsc::Sender<MessageEnvelope>,
    mut rx_send: mpsc::Receiver<MessageEnvelope>,
    sessions: Arc<Mutex<Vec<Weak<WebsocketSessionEntry>>>>,
) -> anyhow::Result<()> {
    // Create the session and initiate websocket handshake
    let mut ws_stream = tokio_tungstenite::accept_async(stream).await?;

    let (tx_close, rx_close) = oneshot::channel::<()>();
    let entry = Arc::new(WebsocketSessionEntry::new(
        tx_send,
        tx_close,
        wallet_services,
    ));

    {
        let mut sessions = sessions.lock().unwrap();
        sessions.retain(|s| s.strong_count() > 0);
        sessions.push(Arc::downgrade(&entry));
    }

    let session = WebsocketSession::new(topic_subscriber_count, peer_addr, entry);

    tokio::select! {
        _ = rx_close =>{
            ws_stream
                .close(Some(CloseFrame {
                    code: CloseCode::Normal,
                    reason: "Shutting down".into(),
                }))
                .await?;
        }
        res = session.run(&mut ws_stream, &mut rx_send) =>{
            res?;
        }
    };

    Ok(())
}

pub fn into_json_vote_summary(v: &VoteSummary) -> JsonVoteSummary {
    JsonVoteSummary {
        representative: Account::from(v.voter).encode_account(),
        timestamp: v.vote_created.to_string(),
        hash: v.hash.to_string(),
        weight: v.weight.to_string_dec(),
    }
}

pub fn into_election_info(value: &ConfirmedElection) -> ElectionInfo {
    ElectionInfo {
        duration: value.election_duration.as_millis().to_string(),
        time: value
            .election_end
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .to_string(),
        tally: value.tally.to_string_dec(),
        final_tally: value.final_tally.to_string_dec(),
        blocks: value.block_count.to_string(),
        voters: value.voter_count.to_string(),
        request_count: 0.to_string(), // currently not supported in RsNano
        votes: None,
    }
}

pub fn into_json_sideband(value: &BlockSideband) -> JsonSideband {
    JsonSideband {
        height: value.height.to_string(),
        local_timestamp: value.timestamp.to_string(),
    }
}
