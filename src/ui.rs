use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap, canvas::Canvas},
    Frame,
};
use std::collections::{HashMap, VecDeque};

pub struct AppState {
    pub snapshots_received: usize,
    pub ready_for_arbitrage: bool,
    pub best_opportunities: Vec<ArbitrageOpportunity>,
    pub best_ever_opportunity: Option<ArbitrageOpportunity>,
    pub node_count: usize,
    pub edge_count: usize,
    pub node_positions: HashMap<String, (f64, f64)>,
    pub edges: Vec<(String, String)>, // List of edges as (from, to) pairs
    pub messages_per_second: f64,
    pub total_messages_received: usize,
    pub logs: VecDeque<String>,
}

#[derive(Clone)]
pub struct ArbitrageOpportunity {
    pub multiplier: f64,
    pub size_usd: f64,
    pub path: String,
}

impl AppState {
    pub fn new(node_count: usize, edge_count: usize) -> Self {
        AppState {
            snapshots_received: 0,
            ready_for_arbitrage: false,
            best_opportunities: Vec::new(),
            best_ever_opportunity: None,
            node_count,
            edge_count,
            node_positions: HashMap::new(),
            edges: Vec::new(),
            messages_per_second: 0.0,
            total_messages_received: 0,
            logs: VecDeque::new(),
        }
    }

    pub fn add_log(&mut self, message: String) {
        self.logs.push_back(message);
        // Keep only the last 100 log messages
        if self.logs.len() > 100 {
            self.logs.pop_front();
        }
    }

    pub fn calculate_node_positions(&mut self, nodes: &[String], edges: &[(String, String)]) {
        let center_x = 50.0;
        let center_y = 50.0;

        // Count the degree (number of edges) for each node
        let mut degrees: HashMap<String, usize> = HashMap::new();
        for node in nodes {
            degrees.insert(node.clone(), 0);
        }
        for (from, to) in edges {
            *degrees.get_mut(from).unwrap() += 1;
            *degrees.get_mut(to).unwrap() += 1;
        }

        // Sort nodes by degree (descending)
        let mut sorted_nodes: Vec<(String, usize)> = degrees.iter()
            .map(|(node, &degree)| (node.clone(), degree))
            .collect();
        sorted_nodes.sort_by(|a, b| b.1.cmp(&a.1));

        // Place high-degree nodes in the center, lower-degree nodes on the periphery
        // Use concentric circles based on degree
        let max_degree = sorted_nodes[0].1 as f64;

        for (node, degree) in sorted_nodes {
            // Normalize degree to 0-1 range (inverted so high degree = center)
            let degree_normalized = 1.0 - (degree as f64 / max_degree);

            // Radius based on degree: high degree = small radius (center), low degree = large radius (edge)
            let radius = 5.0 + degree_normalized * 40.0;

            // Distribute nodes with same degree evenly around their circle
            let same_degree_nodes: Vec<_> = nodes.iter()
                .filter(|n| degrees[*n] == degree)
                .collect();
            let index_in_degree = same_degree_nodes.iter().position(|n| *n == &node).unwrap();
            let total_at_degree = same_degree_nodes.len();

            let angle = 2.0 * std::f64::consts::PI * (index_in_degree as f64) / (total_at_degree as f64);
            let x = center_x + radius * angle.cos();
            let y = center_y + radius * angle.sin();

            self.node_positions.insert(node.clone(), (x, y));
        }
    }
}

pub fn draw_ui(frame: &mut Frame, state: &AppState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),   // Header
            Constraint::Min(10),     // Main area (graph + opportunities)
            Constraint::Length(10),   // Logs
        ])
        .split(frame.area());

    // Header
    draw_header(frame, chunks[0], state);

    // Split main area into graph and opportunities
    let main_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(60),  // Graph
            Constraint::Percentage(40),  // Opportunities
        ])
        .split(chunks[1]);

    // Graph visualization
    draw_graph(frame, main_chunks[0], state);

    // Opportunities panel (moved to right side)
    draw_opportunities(frame, main_chunks[1], state);

    // Logs
    draw_logs(frame, chunks[2], state);
}

fn draw_header(frame: &mut Frame, area: Rect, state: &AppState) {
    let status_color = if state.ready_for_arbitrage {
        Color::Green
    } else {
        Color::Yellow
    };

    let status_text = if state.ready_for_arbitrage {
        "MONITORING"
    } else {
        "INITIALIZING"
    };

    let header = Paragraph::new(Line::from(vec![
        Span::styled("ANTARES ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
        Span::raw("| "),
        Span::styled(status_text, Style::default().fg(status_color).add_modifier(Modifier::BOLD)),
        Span::raw(" | "),
        Span::styled(
            format!("Snapshots: {}/{}", state.snapshots_received, state.edge_count),
            Style::default().fg(Color::White),
        ),
        Span::raw(" | "),
        Span::styled(
            format!("Msgs/sec: {:.1}", state.messages_per_second),
            Style::default().fg(Color::White),
        ),
        Span::raw(" | "),
        Span::styled(
            format!("Total: {}", state.total_messages_received),
            Style::default().fg(Color::White),
        ),
    ]))
    .block(Block::default().borders(Borders::ALL).border_style(Style::default().fg(Color::Cyan)));

    frame.render_widget(header, area);
}

fn draw_graph(frame: &mut Frame, area: Rect, state: &AppState) {
    let canvas = Canvas::default()
        .block(Block::default()
            .title(format!("Network Graph ({} nodes, {} edges)", state.node_count, state.edge_count))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Blue)))
        .x_bounds([0.0, 100.0])
        .y_bounds([0.0, 100.0])
        .paint(|ctx| {
            // Parse best ever opportunity path if it exists
            let best_ever_path_nodes: Vec<String> = if let Some(best_ever) = &state.best_ever_opportunity {
                best_ever.path.split(" > ").map(|s| s.to_string()).collect()
            } else {
                Vec::new()
            };

            // Draw edges first (so they appear behind nodes)
            for (from, to) in &state.edges {
                if let (Some(&(x1, y1)), Some(&(x2, y2))) =
                    (state.node_positions.get(from), state.node_positions.get(to)) {
                    // Check if this edge is part of the best ever opportunity path
                    let in_best_ever_path = if !best_ever_path_nodes.is_empty() {
                        // Check if from and to are consecutive in the path
                        best_ever_path_nodes.windows(2).any(|window| {
                            (&window[0] == from && &window[1] == to) ||
                            (&window[0] == to && &window[1] == from)
                        })
                    } else {
                        false
                    };

                    let color = if in_best_ever_path {
                        Color::Yellow
                    } else {
                        Color::DarkGray
                    };

                    ctx.draw(&ratatui::widgets::canvas::Line {
                        x1,
                        y1,
                        x2,
                        y2,
                        color,
                    });
                }
            }

            // Draw nodes on top of edges
            for (node, &(x, y)) in &state.node_positions {
                // Check if this node is part of the best ever opportunity path
                let in_best_ever_path = best_ever_path_nodes.contains(node);

                let color = if in_best_ever_path {
                    Color::Yellow
                } else {
                    Color::Green
                };

                // Draw node as a circle (using Points for simplicity)
                ctx.draw(&ratatui::widgets::canvas::Circle {
                    x,
                    y,
                    radius: 1.5,
                    color,
                });

                // Draw node label nearby (offset slightly)
                ctx.print(x + 2.0, y, Span::styled(node.clone(), Style::default().fg(color)));
            }
        });

    frame.render_widget(canvas, area);
}

fn draw_opportunities(frame: &mut Frame, area: Rect, state: &AppState) {
    let mut lines = vec![];

    // Show best-ever opportunity at the top
    if let Some(best_ever) = &state.best_ever_opportunity {
        lines.push(Line::from(vec![
            Span::styled("🏆 BEST EVER: ", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
            Span::styled(
                format!("{:.6}x", best_ever.multiplier),
                Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
            ),
            Span::raw(" | "),
            Span::styled(
                format!("${:.2}", best_ever.size_usd),
                Style::default().fg(Color::Cyan),
            ),
        ]));
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(&best_ever.path, Style::default().fg(Color::White)),
        ]));
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled("─".repeat(50), Style::default().fg(Color::DarkGray)),
        ]));
        lines.push(Line::from(""));
    }

    if state.best_opportunities.is_empty() {
        lines.push(Line::from(vec![
            Span::styled("⌛ ", Style::default().fg(Color::Yellow)),
            Span::raw("Waiting for arbitrage opportunities..."),
        ]));
    } else {
        // Show current opportunities
        lines.push(Line::from(vec![
            Span::styled("Current Opportunities:", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
        ]));
        lines.push(Line::from(""));

        for (idx, opp) in state.best_opportunities.iter().enumerate() {
            // Highlight the opportunity
            let color = if opp.multiplier > 1.01 {
                Color::Green
            } else if opp.multiplier > 1.001 {
                Color::Yellow
            } else {
                Color::White
            };

            lines.push(Line::from(vec![
                Span::styled(
                    format!("#{} ", idx + 1),
                    Style::default().fg(Color::Gray),
                ),
                Span::styled(
                    format!("{:.6}x", opp.multiplier),
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                ),
                Span::raw(" | "),
                Span::styled(
                    format!("${:.2}", opp.size_usd),
                    Style::default().fg(Color::Cyan),
                ),
                Span::raw(" | "),
                Span::styled(&opp.path, Style::default().fg(Color::White)),
            ]));
        }
    }

    let title = if state.best_opportunities.is_empty() {
        "Arbitrage Opportunities".to_string()
    } else {
        format!("Arbitrage Opportunities ({})", state.best_opportunities.len())
    };

    let paragraph = Paragraph::new(lines)
        .block(Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Magenta)))
        .wrap(Wrap { trim: true });

    frame.render_widget(paragraph, area);
}

fn draw_logs(frame: &mut Frame, area: Rect, state: &AppState) {
    let mut lines = vec![];

    // Show the most recent logs (up to what fits in the area)
    let max_logs = (area.height.saturating_sub(2)) as usize; // -2 for borders
    let start_idx = if state.logs.len() > max_logs {
        state.logs.len() - max_logs
    } else {
        0
    };

    for log in state.logs.iter().skip(start_idx) {
        let color = if log.contains("⚠️") || log.contains("Gap") || log.contains("stale") {
            Color::Yellow
        } else if log.contains("❌") || log.contains("Failed") || log.contains("Error") {
            Color::Red
        } else {
            Color::Gray
        };
        lines.push(Line::from(Span::styled(log.clone(), Style::default().fg(color))));
    }

    if lines.is_empty() {
        lines.push(Line::from(Span::styled("No logs yet...", Style::default().fg(Color::Gray))));
    }

    let paragraph = Paragraph::new(lines)
        .block(Block::default()
            .title("System Logs")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Gray)))
        .wrap(Wrap { trim: true });

    frame.render_widget(paragraph, area);
}
