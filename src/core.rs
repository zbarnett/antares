use petgraph::graph::{UnGraph, NodeIndex};
use petgraph::visit::EdgeRef;
use std::collections::{HashMap, BTreeMap, VecDeque};
use ordered_float::NotNan;

/// Represents an edge in the trading graph with order book data
pub struct Edge {
    pub base_currency: String,
    pub quote_currency: String,
    // BTreeMap automatically keeps prices sorted
    pub bids: BTreeMap<NotNan<f64>, f64>,  // price -> size
    pub asks: BTreeMap<NotNan<f64>, f64>,  // price -> size
}

impl Edge {
    pub fn new(base_currency: String, quote_currency: String) -> Self {
        Edge {
            base_currency,
            quote_currency,
            bids: BTreeMap::new(),
            asks: BTreeMap::new(),
        }
    }

    pub fn update_bid(&mut self, price: f64, size: f64) {
        if size == 0.0 {
            self.bids.remove(&NotNan::new(price).unwrap());
        } else {
            self.bids.insert(NotNan::new(price).unwrap(), size);
        }
    }

    pub fn update_ask(&mut self, price: f64, size: f64) {
        if size == 0.0 {
            self.asks.remove(&NotNan::new(price).unwrap());
        } else {
            self.asks.insert(NotNan::new(price).unwrap(), size);
        }
    }

    pub fn clear(&mut self) {
        self.bids.clear();
        self.asks.clear();
    }

    /// Get best bid (highest price)
    pub fn get_best_bid(&self) -> Option<(f64, f64)> {
        self.bids.last_key_value().map(|(price, size)| (price.into_inner(), *size))
    }

    /// Get best ask (lowest price)
    pub fn get_best_ask(&self) -> Option<(f64, f64)> {
        self.asks.first_key_value().map(|(price, size)| (price.into_inner(), *size))
    }
}

/// Represents a cycle with its calculated gain
#[derive(Clone)]
pub struct GainCycle {
    pub gain: (f64, f64),
    pub cycle: Vec<NodeIndex>,
}

/// Find a node in the graph by its weight
pub fn find_node_with_weight<N, E>(graph: &UnGraph<N, E>, weight: &N) -> Option<NodeIndex>
where
    N: PartialEq,
{
    graph.node_indices().find(|&node| graph[node] == *weight)
}

/// Compute the top-of-book rate and available liquidity for a single leg.
///
/// Returns `(rate, available_from)` where:
/// - `rate` is how many units of `to_currency` you receive per unit of
///   `from_currency`, with the taker fee already applied.
/// - `available_from` is how much of `from_currency` the best level can absorb,
///   expressed in `from_currency` units.
///
/// Returns `None` if the relevant side of the book is empty or the leg does not
/// correspond to this edge's base/quote pair.
fn leg_top_of_book(
    edge: &Edge,
    from_currency: &str,
    to_currency: &str,
    taker_fee: f64,
) -> Option<(f64, f64)> {
    if from_currency == edge.base_currency && to_currency == edge.quote_currency {
        // Selling base for quote: hit the best bid.
        // Price is quote-per-base; available base is the bid size.
        let (bid_price, bid_size) = edge.get_best_bid()?;
        let rate = bid_price * (1.0 - taker_fee);
        Some((rate, bid_size))
    } else if from_currency == edge.quote_currency && to_currency == edge.base_currency {
        // Buying base with quote: lift the best ask.
        // Ask price is quote-per-base, so rate (base-per-quote) is 1/effective.
        // The ask size is in base units; convert to available quote
        // (from-currency units) by multiplying by the effective price.
        let (ask_price, ask_size) = edge.get_best_ask()?;
        let effective = ask_price * (1.0 + taker_fee);
        if effective <= 0.0 {
            return None;
        }
        let rate = 1.0 / effective;
        let available_from = ask_size * effective;
        Some((rate, available_from))
    } else {
        None
    }
}

/// Calculate the gain for traversing a cycle in the trading graph.
///
/// `taker_fee` is the per-leg taker fee as a fraction (e.g. `0.001` for 0.1%).
///
/// Returns `(multiplier, size)` where `multiplier` is the product of every
/// leg's top-of-book rate (a value > 1.0 means a profitable round trip) and
/// `size` is the achievable trade size in the starting currency's units,
/// limited by the thinnest leg. When the cycle is not profitable, `size` is
/// reported as 0.0.
pub fn calculate_gain(
    cycle: &[NodeIndex],
    graph: &UnGraph<String, Edge>,
    taker_fee: f64,
) -> (f64, f64) {
    // Pass 1: detect profitability from top-of-book rates only.
    let mut multiplier: f64 = 1.0;
    for [from, to] in cycle.array_windows() {
        let edge = graph.edges_connecting(*from, *to).next().unwrap().weight();
        let from_currency = graph.node_weight(*from).unwrap();
        let to_currency = graph.node_weight(*to).unwrap();

        let (rate, _) = match leg_top_of_book(edge, from_currency, to_currency, taker_fee) {
            Some(v) => v,
            None => return (0.0, 0.0),
        };

        if rate <= 0.0 || !rate.is_finite() {
            return (0.0, 0.0);
        }
        multiplier *= rate;
    }

    if !multiplier.is_finite() {
        return (0.0, 0.0);
    }
    if multiplier <= 1.0 {
        return (multiplier, 0.0);
    }

    // Pass 2: compute the achievable size, bounded by the thinnest leg.
    // `size` is tracked in the current leg's from-currency units; after a leg
    // it is converted into the next currency via that leg's rate.
    let mut size: f64 = f64::MAX;
    for [from, to] in cycle.array_windows() {
        let edge = graph.edges_connecting(*from, *to).next().unwrap().weight();
        let from_currency = graph.node_weight(*from).unwrap();
        let to_currency = graph.node_weight(*to).unwrap();

        let (rate, available_from) =
            match leg_top_of_book(edge, from_currency, to_currency, taker_fee) {
                Some(v) => v,
                None => return (0.0, 0.0),
            };

        size = size.min(available_from) * rate;

        if !size.is_finite() || size <= 0.0 {
            return (0.0, 0.0);
        }
    }

    (multiplier, size)
}

/// Format a cycle as a human-readable path string
pub fn print_cycle(cycle: &[NodeIndex], graph: &UnGraph<String, Edge>) -> String {
    let mut builder = String::new();

    let start_label = graph.node_weight(cycle[0]).unwrap();
    builder.push_str(start_label);

    for [_from, to] in cycle.array_windows() {
        let to_label = graph.node_weight(*to).unwrap();
        builder.push_str(&format!(" > {}", to_label));
    }

    builder
}

/// Convert an amount in a given currency to USD using BFS path finding
pub fn convert_to_usd(graph: &UnGraph<String, Edge>, currency_node: NodeIndex, amount: f64) -> f64 {
    let currency_name = graph.node_weight(currency_node).unwrap();

    if currency_name == "USD" {
        return amount;
    }

    let usd_node = find_node_with_weight(graph, &"USD".to_string())
        .expect("USD node not found in graph");

    let mut queue = VecDeque::new();
    let mut visited = HashMap::new();

    queue.push_back(currency_node);
    visited.insert(currency_node, (None, 1.0)); // (parent, cumulative_rate)

    while let Some(current) = queue.pop_front() {
        if current == usd_node {
            let mut node = current;

            while let Some((parent, _)) = visited.get(&node) {
                if let Some(p) = parent {
                    node = *p;
                } else {
                    break;
                }
            }

            return amount * visited.get(&current).unwrap().1;
        }

        for edge in graph.edges(current) {
            let target = edge.target();

            if visited.contains_key(&target) {
                continue;
            }

            let from_currency = graph.node_weight(current).unwrap();
            let to_currency = graph.node_weight(target).unwrap();
            let edge_weight = edge.weight();

            let price_opt = if from_currency == &edge_weight.base_currency && to_currency == &edge_weight.quote_currency {
                edge_weight.get_best_bid()
            } else if from_currency == &edge_weight.quote_currency && to_currency == &edge_weight.base_currency {
                if let Some((ask_price, ask_size)) = edge_weight.get_best_ask() {
                    Some((1.0 / ask_price, ask_size))
                } else {
                    None
                }
            } else {
                None
            };

            if let Some((price, _)) = price_opt {
                let current_rate = visited.get(&current).unwrap().1;
                let new_rate = current_rate * price;
                visited.insert(target, (Some(current), new_rate));
                queue.push_back(target);
            }
        }
    }

    panic!("No path found from {} to USD", currency_name);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build an edge for `base`/`quote` with the given bid and ask levels.
    fn make_edge(
        base: &str,
        quote: &str,
        bids: &[(f64, f64)],
        asks: &[(f64, f64)],
    ) -> Edge {
        let mut edge = Edge::new(base.to_string(), quote.to_string());
        for &(price, size) in bids {
            edge.update_bid(price, size);
        }
        for &(price, size) in asks {
            edge.update_ask(price, size);
        }
        edge
    }

    /// Build a 3-currency triangle A -> B -> C -> A.
    ///
    /// Leg layout (matching the cycle `[A, B, C, A]`):
    /// - A -> B : edge base=B quote=A, traversed quote->base (uses asks)
    /// - B -> C : edge base=C quote=B, traversed quote->base (uses asks)
    /// - C -> A : edge base=C quote=A, traversed base->quote (uses bids)
    ///
    /// Returns the graph and the closed cycle `[A, B, C, A]`.
    fn triangle(
        ab_asks: &[(f64, f64)],
        bc_asks: &[(f64, f64)],
        ca_bids: &[(f64, f64)],
    ) -> (UnGraph<String, Edge>, Vec<NodeIndex>) {
        let mut graph = UnGraph::<String, Edge>::new_undirected();
        let a = graph.add_node("A".to_string());
        let b = graph.add_node("B".to_string());
        let c = graph.add_node("C".to_string());

        graph.add_edge(a, b, make_edge("B", "A", &[], ab_asks));
        graph.add_edge(b, c, make_edge("C", "B", &[], bc_asks));
        graph.add_edge(c, a, make_edge("C", "A", ca_bids, &[]));

        (graph, vec![a, b, c, a])
    }

    #[test]
    fn detects_profitable_triangle_with_correct_size() {
        // A->B ask 100@5  : rate 1/100, avail 500 (quote units)
        // B->C ask 10@50  : rate 1/10,  avail 500
        // C->A bid 1500@2 : rate 1500,  avail 2 (base units)
        // multiplier = 0.01 * 0.1 * 1500 = 1.5
        let (graph, cycle) = triangle(&[(100.0, 5.0)], &[(10.0, 50.0)], &[(1500.0, 2.0)]);
        let (multiplier, size) = calculate_gain(&cycle, &graph, 0.0);

        assert!((multiplier - 1.5).abs() < 1e-9, "multiplier was {}", multiplier);
        // Size walk: MAX -> min(500)*0.01 = 5 -> min(5,500)*0.1 = 0.5
        //            -> min(0.5,2)*1500 = 750
        assert!((size - 750.0).abs() < 1e-9, "size was {}", size);
    }

    #[test]
    fn no_opportunity_when_spread_eats_it() {
        // Same as above but the closing bid is only 900, so
        // multiplier = 0.01 * 0.1 * 900 = 0.9 (< 1.0): not profitable.
        let (graph, cycle) = triangle(&[(100.0, 5.0)], &[(10.0, 50.0)], &[(900.0, 2.0)]);
        let (multiplier, size) = calculate_gain(&cycle, &graph, 0.0);

        assert!((multiplier - 0.9).abs() < 1e-9, "multiplier was {}", multiplier);
        assert_eq!(size, 0.0, "unprofitable cycle must report zero size");
    }

    #[test]
    fn uses_top_of_book_not_deep_average() {
        // Add deep, worse levels behind each best price. Asks get worse by
        // going higher; bids get worse by going lower. Top of book is
        // unchanged, so the result must match the clean triangle exactly.
        let (graph, cycle) = triangle(
            &[(100.0, 5.0), (200.0, 1000.0)],
            &[(10.0, 50.0), (40.0, 1000.0)],
            &[(1500.0, 2.0), (1000.0, 1000.0)],
        );
        let (multiplier, size) = calculate_gain(&cycle, &graph, 0.0);

        assert!((multiplier - 1.5).abs() < 1e-9, "multiplier was {}", multiplier);
        assert!((size - 750.0).abs() < 1e-9, "size was {}", size);
    }

    #[test]
    fn taker_fee_can_remove_the_edge() {
        // quote->base leg with a 1% taker fee: ask 100 becomes effective 101.
        let edge = make_edge("B", "A", &[], &[(100.0, 5.0)]);
        let (rate, avail) = leg_top_of_book(&edge, "A", "B", 0.01).unwrap();

        assert!((rate - 1.0 / 101.0).abs() < 1e-12, "rate was {}", rate);
        // available_from = ask_size * effective = 5 * 101 = 505 (quote units)
        assert!((avail - 505.0).abs() < 1e-9, "avail was {}", avail);
    }

    #[test]
    fn missing_book_side_yields_zero() {
        // The closing leg needs a bid, but there is none.
        let (graph, cycle) = triangle(&[(100.0, 5.0)], &[(10.0, 50.0)], &[]);
        let (multiplier, size) = calculate_gain(&cycle, &graph, 0.0);

        assert_eq!(multiplier, 0.0);
        assert_eq!(size, 0.0);
    }

    #[test]
    fn fee_is_applied_per_leg() {
        // The 1.5x cycle, but with a per-leg fee. Two quote->base legs pay
        // (1 + f) on the ask, one base->quote leg pays (1 - f) on the bid:
        //   multiplier = 1.5 * (1 - f) / (1 + f)^2
        let f = 0.001;
        let (graph, cycle) = triangle(&[(100.0, 5.0)], &[(10.0, 50.0)], &[(1500.0, 2.0)]);
        let (multiplier, _) = calculate_gain(&cycle, &graph, f);

        let expected = 1.5 * (1.0 - f) / ((1.0 + f) * (1.0 + f));
        assert!((multiplier - expected).abs() < 1e-12, "multiplier was {}", multiplier);
    }
}
