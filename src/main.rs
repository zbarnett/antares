mod graph_cycles;

use petgraph::graph::{DiGraph, NodeIndex};
use graph_cycles::Cycles;
use reqwest;
use serde::Deserialize;
use std::collections::HashMap;
use tungstenite::{connect, Message};

#[derive(Debug, Deserialize)]
struct CoinbasePair {
    id: String,
    base_currency: String,
    quote_currency: String,
	status: String,
}

#[derive(Debug, Deserialize)]
struct TickerEntry {
    product_id: String,
    best_bid: String,
    best_ask: String,
}

fn fetch_trading_pairs() -> Vec<CoinbasePair> {
    let url = "https://api.exchange.coinbase.com/products";
    let response = reqwest::blocking::get(url).unwrap().json::<Vec<CoinbasePair>>().unwrap();

	response.into_iter().filter(|x| x.status == "online").collect()
}

fn fetch_exchange_rates(pairs: &[CoinbasePair], graph: &mut DiGraph::<String, f64>) {
    println!("finding cycles");

    let cycles = &graph.cycles();

    println!("Starting websocket client to stay up to date...");

    // only get rates for trading pairs that are in the graph
    let filtered_pairs: Vec<&CoinbasePair> = pairs.into_iter().filter(|x| node_with_weight(&graph, &x.base_currency) && node_with_weight(&graph, &x.quote_currency)).collect();
    let filtered_ids = filtered_pairs.into_iter().map(|x| format!("\"{}\"", x.id).into()).collect::<Vec<String>>().join(", ");
    println!("Pairs we're watching: {filtered_ids}");

    let (mut socket, _) = connect("wss://ws-feed.exchange.coinbase.com").expect("Can't connect");
    println!("Connected to the websocket feed");

    socket.send(Message::Text(format!("{{ \"type\": \"subscribe\", \"product_ids\": [{filtered_ids}], \"channels\": [\"ticker\"] }}").into())).expect("Error sending message");
    println!("Sent subscribe message");

    loop {
        let msg = socket.read().expect("Error reading message");

        if let Message::Text(text_msg) = msg {
            if let Ok(entry) = serde_json::from_str::<TickerEntry>(&text_msg) {
                let (base_str, quote_str) = entry.product_id.split_once("-").unwrap();
                let ask = entry.best_ask.parse::<f64>().unwrap();
                let bid = entry.best_bid.parse::<f64>().unwrap();
                let base = find_node_with_weight(&graph, &base_str.to_string()).unwrap();
                let quote = find_node_with_weight(&graph, &quote_str.to_string()).unwrap();
                graph.update_edge(base, quote, 1.0/bid);
                graph.update_edge(quote, base, ask);

                let gain_cycles: Vec<GainCycle> = cycles.into_iter().map(|x| GainCycle { gain: calculate_gain(&x, &graph), cycle: x.clone()}).collect();
                let best_deal = gain_cycles.iter().max_by(|a, b| a.gain.partial_cmp(&b.gain).unwrap()).unwrap();
                println!("net gain {}x for path {}", best_deal.gain, print_cycle(&best_deal.cycle, &graph));
            }
            else {
                println!("Non ticker entry: {text_msg}");
            }
        }
    }
}

fn find_node_with_weight<N, E>(graph: &DiGraph<N, E>, weight: &N) -> Option<NodeIndex>
where
    N: PartialEq,
{
    graph.node_indices().find(|&node| graph[node] == *weight)
}

fn node_with_weight<N, E>(graph: &DiGraph<N, E>, weight: &N) -> bool
where
    N: PartialEq,
{
    if let Some(_) = graph.node_indices().find(|&node| graph[node] == *weight) {
        true
    } else {
        false
    }
}

fn main() {
    let trading_pairs = fetch_trading_pairs();

    println!("loaded {} online trading pairs", &trading_pairs.len());

    // then build graph with dummy rates
    let mut graph = DiGraph::<String, f64>::new();
    let mut node_map = HashMap::new();
    // Add nodes to graph
    for trading_pair in &trading_pairs {
        // skip view-only currency pairs for now (until I can figure out how to get access to trade them)
        if trading_pair.base_currency == "EUR" || trading_pair.quote_currency == "EUR" || trading_pair.base_currency == "GBP" || trading_pair.quote_currency == "GBP" {
            continue
        }

        node_map.entry(trading_pair.base_currency.clone()).or_insert_with(|| graph.add_node(trading_pair.base_currency.clone()));
        node_map.entry(trading_pair.quote_currency.clone()).or_insert_with(|| graph.add_node(trading_pair.quote_currency.clone()));
    }

    // Add edges
    for trading_pair in &trading_pairs {
        // skip view-only currency pairs for now (until I can figure out how to get access to trade them)
        if trading_pair.base_currency == "EUR" || trading_pair.quote_currency == "EUR" || trading_pair.base_currency == "GBP" || trading_pair.quote_currency == "GBP" {
            continue
        }

        let base = node_map[&trading_pair.base_currency];
        let quote = node_map[&trading_pair.quote_currency];
        graph.add_edge(base, quote, 0.0);
        graph.add_edge(quote, base, 0.0);
    }

    println!("built graph with {} nodes and {} edges", graph.node_count(), graph.edge_count());

    // Collect nodes with exactly one outgoing edge
    let nodes_to_remove: Vec<_> = graph.node_indices()
        .filter(|&node| graph.edges_directed(node, petgraph::Direction::Outgoing).count() == 1)
        .collect();

    println!("found {} nodes with only one outgoing edge so we can't use it for triangle arbitrage", nodes_to_remove.len());

    // Remove them
    for node_idx in nodes_to_remove.into_iter().rev() {
        graph.remove_node(node_idx);
    }

    println!("trimmed down to {} nodes and {} edges", graph.node_count(), graph.edge_count());

    // update edges with actual rates now
    fetch_exchange_rates(&trading_pairs, &mut graph);
}

struct GainCycle {
    gain: f64,
    cycle: Vec<NodeIndex>,
}

fn calculate_gain(cycle: &Vec<NodeIndex>, graph: &DiGraph::<String, f64>) -> f64 {
    let mut gain: f64 = 1.0;

    for window in cycle.windows(2) {
        let edge = graph.edges_connecting(window[0], window[1]).next().unwrap();

        gain *= edge.weight() * (1.0 - 0.012) // factor in taker fee of 1.2%
    }
    
    gain
}

fn print_cycle(cycle: &Vec<NodeIndex>, graph: &DiGraph::<String, f64>) -> String {
    let mut builder = String::new();

    for window in cycle.windows(2) {
        let edge = graph.edges_connecting(window[0], window[1]).next().unwrap();
        let from_label = graph.node_weight(window[0]).unwrap();
        let to_label = graph.node_weight(window[1]).unwrap();

        builder.push_str(&format!("{} -- {} --> {}, ", from_label, edge.weight(), to_label));
    }

    builder
}