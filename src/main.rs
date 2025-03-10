mod graph_cycles;

use petgraph::graph::{DiGraph, NodeIndex};
use petgraph::dot::Dot;
use graph_cycles::Cycles;
use reqwest;
use reqwest::header::USER_AGENT;
use serde::Deserialize;
use std::collections::HashMap;

#[derive(Debug, Deserialize)]
struct CoinbasePair {
    id: String,
    base_currency: String,
    quote_currency: String,
	status: String,
}

#[derive(Debug, Deserialize)]
struct CoinbaseTicker {
    ask: String,
    bid: String,
}

fn fetch_trading_pairs() -> Vec<CoinbasePair> {
    let url = "https://api.exchange.coinbase.com/products";
    let response = reqwest::blocking::get(url).unwrap().json::<Vec<CoinbasePair>>().unwrap();

	response.into_iter().filter(|x| x.status == "online").collect()
}

fn fetch_exchange_rates(pairs: &[CoinbasePair], graph: &mut DiGraph::<String, f64>) {
    let client = reqwest::blocking::Client::new();

    let filtered_pairs: Vec<&CoinbasePair> = pairs.into_iter().filter(|x| node_with_weight(&graph, &x.base_currency) && node_with_weight(&graph, &x.quote_currency)).collect();

    // only get rates for trading pairs that are in the graph
    for pair in filtered_pairs {
        let url = format!("https://api.exchange.coinbase.com/products/{}/ticker", pair.id);
        let response = client.get(&url).header(USER_AGENT, "My Rust Program 1.0").send().unwrap();
		let ticker = response.json::<CoinbaseTicker>().unwrap();
		let ask = ticker.ask.parse::<f64>().unwrap();
        let bid = ticker.bid.parse::<f64>().unwrap();
        let base = find_node_with_weight(&graph, &pair.base_currency).unwrap();
        let quote = find_node_with_weight(&graph, &pair.quote_currency).unwrap();
        graph.update_edge(base, quote, 1.0/bid);
        graph.update_edge(quote, base, ask);
		println!("{} ask {} bid {}", pair.id, ticker.ask, ticker.bid);
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
        node_map.entry(trading_pair.base_currency.clone()).or_insert_with(|| graph.add_node(trading_pair.base_currency.clone()));
        node_map.entry(trading_pair.quote_currency.clone()).or_insert_with(|| graph.add_node(trading_pair.quote_currency.clone()));
    }

    // Add edges
    for trading_pair in &trading_pairs {
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

	//println!("{}", Dot::new(&graph));

	println!("finding cycles");

	// print each cycle in turn
    let cycles = graph.cycles();

    println!("found {} cycles", cycles.len());

    let mut gain_cycles: Vec<GainCycle> = cycles.into_iter().map(|x| GainCycle { gain: calculate_gain(&x, &graph), cycle: x.clone()}).collect();
    gain_cycles.sort_by(|a, b| a.gain.partial_cmp(&b.gain).unwrap());
    for gain_cycle in gain_cycles {
        if gain_cycle.gain > 1.0 {
            println!("net gain {}x for path {}", gain_cycle.gain, print_cycle(&gain_cycle.cycle, &graph));
        }
    }
}

struct GainCycle {
    gain: f64,
    cycle: Vec<NodeIndex>,
}

fn calculate_gain(cycle: &Vec<NodeIndex>, graph: &DiGraph::<String, f64>) -> f64 {
    let mut gain: f64 = 1.0;

    for window in cycle.windows(2) {
        let edge = graph.edges_connecting(window[0], window[1]).next().unwrap();

        gain *= edge.weight() * 0.996 // factor in taker fee
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