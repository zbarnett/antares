use serde::{Deserialize, Serialize};
use rand::{distributions::Alphanumeric, Rng};
use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use chrono::Utc;
use petgraph::graph::{UnGraph, NodeIndex};
use tungstenite::{connect, Message, stream::MaybeTlsStream};
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::time::Duration;

use crate::core::{Edge, GainCycle, find_node_with_weight, calculate_gain, print_cycle, convert_to_usd};

/// Coinbase taker fee as a fraction (0.1%). Applied to every leg of a cycle.
const TAKER_FEE: f64 = 0.1 / 100.0;

// ============================================================================
// API Data Structures
// ============================================================================

#[derive(Debug, Deserialize)]
struct CoinbasePair {
    base_currency: String,
    quote_currency: String,
    status: String,
}

#[derive(Debug, Deserialize)]
struct AdvancedAPIEntry {
    channel: String,
    events: Vec<EventEntry>,
    sequence_num: u64,
}

#[derive(Debug, Deserialize)]
struct EventEntry {
    // Non-data channels (e.g. "subscriptions") send events with a different
    // shape, so default these fields to let those messages parse cleanly. We
    // only ever read them for the "l2_data" channel.
    #[serde(default)]
    r#type: String,
    #[serde(default)]
    product_id: String,
    #[serde(default)]
    updates: Vec<L2Update>,
}

#[derive(Debug, Deserialize)]
struct L2Update {
    side: String,
    price_level: String,
    new_quantity: String,
}

/// A Coinbase level2 websocket connection.
type Feed = tungstenite::WebSocket<MaybeTlsStream<std::net::TcpStream>>;

// ============================================================================
// JWT Authentication
// ============================================================================

#[derive(Serialize)]
struct Claims<'a> {
    iss: &'a str,
    nbf: i64,
    exp: i64,
    sub: &'a str,
}

/// Raw shape of the Coinbase CDP key file as downloaded from Coinbase.
#[derive(Deserialize)]
struct ApiKeyFile {
    name: String,
    #[serde(rename = "privateKey")]
    private_key: String,
}

/// Coinbase API credentials with the private key normalized to PKCS#8 PEM
/// (the only EC format `jsonwebtoken` accepts).
pub struct Credentials {
    name: String,
    private_key_pkcs8: String,
}

/// Load API credentials from a JSON key file kept out of version control.
///
/// The path defaults to `coinbase_api_key.json` in the working directory and
/// can be overridden with the `COINBASE_API_KEY_FILE` environment variable. The
/// file is the one Coinbase hands you: `{"name": "...", "privateKey": "..."}`.
/// The key may be SEC1 (`BEGIN EC PRIVATE KEY`) or PKCS#8 (`BEGIN PRIVATE KEY`);
/// either is normalized to PKCS#8.
pub fn load_credentials() -> Credentials {
    use p256::pkcs8::{DecodePrivateKey, EncodePrivateKey, LineEnding};
    use p256::SecretKey;

    let path = std::env::var("COINBASE_API_KEY_FILE")
        .unwrap_or_else(|_| "coinbase_api_key.json".to_string());
    let contents = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("Could not read Coinbase API key file '{}': {}", path, e));
    let file: ApiKeyFile = serde_json::from_str(&contents)
        .unwrap_or_else(|e| panic!("Could not parse Coinbase API key file '{}': {}", path, e));

    let key = SecretKey::from_sec1_pem(&file.private_key)
        .or_else(|_| SecretKey::from_pkcs8_pem(&file.private_key))
        .expect("private key is not a valid EC key (SEC1 or PKCS#8 PEM)");
    let private_key_pkcs8 = key
        .to_pkcs8_pem(LineEnding::LF)
        .expect("failed to re-encode key as PKCS#8")
        .to_string();

    Credentials {
        name: file.name,
        private_key_pkcs8,
    }
}

fn generate_jwt(credentials: &Credentials) -> String {
    let now = Utc::now().timestamp();
    let claims = Claims {
        iss: "cdp",
        nbf: now,
        exp: now + 120,
        sub: &credentials.name,
    };

    let nonce: String = rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(16)
        .map(char::from)
        .collect();

    let mut header = Header::new(Algorithm::ES256);
    header.kid = Some(credentials.name.clone());
    header.nonce = Some(nonce);

    let encoding_key = EncodingKey::from_ec_pem(credentials.private_key_pkcs8.as_bytes())
        .expect("invalid PKCS#8 EC private key");
    encode(&header, &claims, &encoding_key).expect("failed to encode JWT")
}

// ============================================================================
// API Functions
// ============================================================================

/// Fetch all currently-online trading pairs from the Coinbase REST API.
fn fetch_trading_pairs() -> Vec<CoinbasePair> {
    let client = reqwest::blocking::Client::builder().user_agent("Antares/0.1").build().unwrap();
    println!("Requesting current products from REST API...");
    let response = client.get("https://api.exchange.coinbase.com/products").send().unwrap();
    let resp_text = response.text().unwrap();

    match serde_json::from_str::<Vec<CoinbasePair>>(&resp_text) {
        Err(e) => panic!("{}", e),
        Ok(res) => res.into_iter().filter(|x| x.status == "online").collect(),
    }
}

/// Build the trading graph from Coinbase's online products: one node per
/// currency, one edge per trading pair. Currencies that touch only a single
/// pair can't be part of a cycle, so they're pruned.
pub fn build_graph() -> UnGraph<String, Edge> {
    let pairs = fetch_trading_pairs();

    let mut graph = UnGraph::<String, Edge>::new_undirected();
    let mut node_map: HashMap<String, NodeIndex> = HashMap::new();

    for pair in &pairs {
        node_map
            .entry(pair.base_currency.clone())
            .or_insert_with(|| graph.add_node(pair.base_currency.clone()));
        node_map
            .entry(pair.quote_currency.clone())
            .or_insert_with(|| graph.add_node(pair.quote_currency.clone()));
    }

    for pair in &pairs {
        let base = node_map[&pair.base_currency];
        let quote = node_map[&pair.quote_currency];
        graph.add_edge(
            base,
            quote,
            Edge::new(pair.base_currency.clone(), pair.quote_currency.clone()),
        );
    }

    // Drop dead-end currencies (degree 1) — they can't participate in a cycle.
    let dead_ends: Vec<_> = graph
        .node_indices()
        .filter(|&node| graph.edges(node).count() == 1)
        .collect();
    for node in dead_ends.into_iter().rev() {
        graph.remove_node(node);
    }

    graph
}

// ============================================================================
// Order Book Processing
// ============================================================================

fn process_order_book_updates(
    graph: &mut UnGraph<String, Edge>,
    base: NodeIndex,
    quote: NodeIndex,
    updates: &[L2Update],
) {
    if let Some(edge_ref) = graph.edge_weight_mut(graph.find_edge(base, quote).unwrap()) {
        for update in updates {
            let price = update.price_level.parse::<f64>().unwrap();
            let size = update.new_quantity.parse::<f64>().unwrap();

            if update.side == "bid" {
                edge_ref.update_bid(price, size);
            } else if update.side == "offer" {
                edge_ref.update_ask(price, size);
            }
        }

        // Invariant check: once the entire batch is applied, the book must not be
        // crossed or locked (best bid < best ask always holds for a correct book).
        // A violation here is a definitive tracking bug, not timing noise.
        if let (Some((best_bid, _)), Some((best_ask, _))) =
            (edge_ref.get_best_bid(), edge_ref.get_best_ask())
            && best_bid >= best_ask {
                eprintln!(
                    "CROSSED BOOK on {}-{}: best bid {} >= best ask {}",
                    edge_ref.base_currency, edge_ref.quote_currency, best_bid, best_ask
                );
            }
    }
}

fn handle_snapshot_event(
    event: &EventEntry,
    graph: &mut UnGraph<String, Edge>,
    base: NodeIndex,
    quote: NodeIndex,
    snapshots_received: &mut HashSet<String>,
    total_products: usize,
) {
    if snapshots_received.insert(event.product_id.clone())
        && snapshots_received.len() == total_products {
            println!("All {} snapshots received — monitoring for arbitrage", total_products);
        }

    if let Some(edge_ref) = graph.edge_weight_mut(graph.find_edge(base, quote).unwrap()) {
        edge_ref.clear();
    }

    process_order_book_updates(graph, base, quote, &event.updates);
}

// ============================================================================
// WebSocket Feed
// ============================================================================

/// Apply a short read timeout so the polling loop never blocks on one socket.
fn set_read_timeout(socket: &mut Feed) {
    let result = match socket.get_mut() {
        MaybeTlsStream::Plain(stream) => {
            stream.set_read_timeout(Some(Duration::from_millis(10)))
        }
        MaybeTlsStream::NativeTls(tls_stream) => {
            tls_stream.get_ref().set_read_timeout(Some(Duration::from_millis(10)))
        }
        _ => {
            eprintln!("Warning: unsupported stream type for setting read timeout");
            return;
        }
    };
    if let Err(e) = result {
        eprintln!("Warning: could not set read timeout: {}", e);
    }
}

/// Connect to Coinbase WebSocket and process exchange rate updates
pub fn fetch_exchange_rates(
    graph: &mut UnGraph<String, Edge>,
    cycles: &[Vec<NodeIndex>],
) {
    // Every edge in the (already pruned) graph is a product we want to stream.
    // Coinbase product IDs are "BASE-QUOTE".
    let product_ids: Vec<String> = graph
        .edge_indices()
        .map(|e| {
            let edge = graph.edge_weight(e).unwrap();
            format!("{}-{}", edge.base_currency, edge.quote_currency)
        })
        .collect();
    let total_products = product_ids.len();

    // Chunk product IDs into groups of 30 (the max products supported per socket).
    let chunks: Vec<Vec<String>> = product_ids
        .chunks(30)
        .map(|chunk| chunk.to_vec())
        .collect();

    // Load credentials once, up front, so a misconfigured key fails fast.
    let credentials = load_credentials();

    // Create a websocket connection for each chunk
    let mut sockets: Vec<Feed> = Vec::new();
    for chunk in chunks.iter() {
        let (mut socket, _) = connect("wss://advanced-trade-ws.coinbase.com")
            .expect("Can't connect");

        set_read_timeout(&mut socket);

        let subscribe_msg = json!({
            "type": "subscribe",
            "product_ids": chunk,
            "channel": "level2",
            "jwt": generate_jwt(&credentials),
        });

        socket.send(Message::Text(subscribe_msg.to_string().into()))
            .expect("Error sending message");

        sockets.push(socket);
    }

    // Track which product_ids have received their initial snapshot
    let mut snapshots_received = HashSet::new();

    // Track last sequence number per socket to detect gaps
    let mut last_sequences: HashMap<usize, u64> = HashMap::new();

    // Track the best multiplier seen so far.
    let mut best_multiplier_ever: Option<f64> = None;

    // Pre-allocate gain cycles storage
    let mut gain_cycles: Vec<GainCycle> = cycles.iter()
        .map(|c| GainCycle { cycle: c.clone(), gain: (0.0, 0.0) })
        .collect();

    loop {
        let mut data_changed = false;

        // Poll each socket and drain all available messages
        for (socket_idx, socket) in sockets.iter_mut().enumerate() {
            loop {
                let msg = match socket.read() {
                    Ok(msg) => msg,
                    Err(e) => {
                        if let tungstenite::Error::Io(ref io_err) = e
                            && (io_err.kind() == std::io::ErrorKind::WouldBlock ||
                               io_err.kind() == std::io::ErrorKind::TimedOut) {
                                break;
                            }
                        panic!("Error reading from socket {}: {:?}", socket_idx, e);
                    }
                };

                if let Message::Text(text_msg) = msg {
                    match serde_json::from_str::<AdvancedAPIEntry>(&text_msg) {
                        Ok(entry) => {
                            // Check for sequence gaps
                            if let Some(&last_seq) = last_sequences.get(&socket_idx)
                                && entry.sequence_num != last_seq + 1 {
                                    eprintln!("Gap detected on socket {}: expected {}, got {} (gap of {})",
                                        socket_idx, last_seq + 1, entry.sequence_num,
                                        entry.sequence_num - last_seq - 1);
                                }
                            last_sequences.insert(socket_idx, entry.sequence_num);

                            // Only the level2 channel carries order book events.
                            // Other channels (e.g. "subscriptions") share the same
                            // sequence stream but have a different event shape.
                            if entry.channel == "l2_data" {
                                for event in entry.events {
                                    let (base_str, quote_str) = event.product_id.split_once("-").unwrap();
                                    let base = find_node_with_weight(graph, &base_str.to_string()).unwrap();
                                    let quote = find_node_with_weight(graph, &quote_str.to_string()).unwrap();

                                    if event.r#type == "snapshot" {
                                        handle_snapshot_event(&event, graph, base, quote, &mut snapshots_received, total_products);
                                    } else if event.r#type == "update" {
                                        data_changed = true;
                                        process_order_book_updates(graph, base, quote, &event.updates);
                                    } else {
                                        eprintln!("Unknown event type: {}", event.r#type);
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            eprintln!("Failed to parse message from socket {}: {}\nMessage: {}",
                                socket_idx, e, &text_msg[..text_msg.len().min(200)]);
                        }
                    }
                } else {
                    eprintln!("Received non-text message from socket {}: {:?}", socket_idx, msg);
                }
            }
        }

        // Only look for arbitrage opportunities once all snapshots are loaded AND data changed
        let is_ready = snapshots_received.len() == total_products;
        if is_ready && data_changed {
            // Recalculate gains for all cycles
            for gc in &mut gain_cycles {
                gc.gain = calculate_gain(&gc.cycle, graph, TAKER_FEE);
            }

            // Rank by multiplier (most profitable round trip), NOT by trade size.
            // Real mispricings are usually tiny in size, so size-ranking buries
            // them behind near-1.0x cycles with deep books.
            let top = gain_cycles.iter().max_by(|a, b| {
                a.gain.0
                    .partial_cmp(&b.gain.0)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });

            if let Some(top) = top {
                let multiplier = top.gain.0;

                // Surface a new all-time-best profitable cycle immediately.
                let is_new_best_ever = multiplier > 1.0
                    && best_multiplier_ever.is_none_or(|b| multiplier > b);
                if is_new_best_ever {
                    best_multiplier_ever = Some(multiplier);
                    let size_usd = convert_to_usd(graph, top.cycle[0], top.gain.1);
                    let path = print_cycle(&top.cycle, graph);
                    println!("NEW BEST EVER: {:.6}x | ${:.2} | {}", multiplier, size_usd, path);
                }
            }
        }
    }
}
