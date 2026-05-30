mod graph_cycles;
mod coinbase;
mod core;

use graph_cycles::Cycles;

fn main() {
    println!("Starting Antares...");

    // All Coinbase-specific work (REST fetch, graph construction) lives in coinbase.rs.
    let mut graph = coinbase::build_graph();
    println!(
        "Graph built: {} currencies, {} trading pairs",
        graph.node_count(),
        graph.edge_count()
    );

    // Pre-compute cycles (they never change after the graph is built).
    let cycles = graph.cycles();
    println!("Found {} cycles", cycles.len());

    // Run the websocket feed, logging straight to the terminal.
    coinbase::fetch_exchange_rates(&mut graph, &cycles);
}
