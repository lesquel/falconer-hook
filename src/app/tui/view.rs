//! Rendering of the TUI state. No I/O: draws into a ratatui `Frame`.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Tabs, Wrap};
use ratatui::Frame;

use super::state::{App, Mode, Screen, UNINSTALL_WORD};

const GLOBAL_KEYS: &str = "Tab/1-7 screens  q quit";

pub fn draw(f: &mut Frame, app: &App) {
    let [tabs, body, footer] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(3),
        Constraint::Length(3),
    ])
    .areas(f.area());

    let titles: Vec<Line> = Screen::ALL
        .iter()
        .enumerate()
        .map(|(i, s)| Line::from(format!("{} {}", i + 1, s.title())))
        .collect();
    f.render_widget(
        Tabs::new(titles)
            .select(app.screen.index())
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" falconer-hook "),
            )
            .highlight_style(Style::default().add_modifier(Modifier::BOLD | Modifier::REVERSED)),
        tabs,
    );

    match app.screen {
        Screen::Status => status(f, app, body),
        Screen::Agents => agents(f, app, body),
        Screen::Devices => devices(f, app, body),
        Screen::Pair => pair(f, app, body),
        Screen::Integrations => integrations(f, app, body),
        Screen::Setup => setup(f, app, body),
        Screen::Uninstall => uninstall(f, app, body),
    }

    let msg = app.message.as_ref().map_or(Line::from(""), |m| {
        Line::from(Span::styled(
            m.text.clone(),
            Style::default().fg(if m.error { Color::Red } else { Color::Green }),
        ))
    });
    f.render_widget(
        Paragraph::new(vec![
            msg,
            Line::from(format!("{}  {GLOBAL_KEYS}", app.screen.keys())),
        ])
        .block(Block::default().borders(Borders::TOP)),
        footer,
    );

    modal(f, app);
}

fn block(title: &str) -> Block<'_> {
    Block::default()
        .borders(Borders::ALL)
        .title(format!(" {title} "))
}

fn list(f: &mut Frame, area: Rect, title: &str, items: Vec<ListItem>, sel: Option<usize>) {
    let empty = items.is_empty();
    let items = if empty {
        vec![ListItem::new("(none)")]
    } else {
        items
    };
    let mut state = ListState::default().with_selected(if empty { None } else { sel });
    f.render_stateful_widget(
        List::new(items)
            .block(block(title))
            .highlight_symbol("> ")
            .highlight_style(Style::default().add_modifier(Modifier::REVERSED)),
        area,
        &mut state,
    );
}

fn status(f: &mut Frame, app: &App, area: Rect) {
    let mut lines = vec![Line::from(format!(
        "Version {}   Health: {}",
        app.version.as_deref().unwrap_or("?"),
        match app.healthy {
            Some(true) => "ok",
            Some(false) => "needs attention",
            None => "unknown",
        }
    ))];
    for c in &app.checks {
        let (mark, color) = match c.status.as_str() {
            "ok" => ("ok  ", Color::Green),
            "warn" => ("warn", Color::Yellow),
            _ => ("FAIL", Color::Red),
        };
        lines.push(Line::from(vec![
            Span::styled(format!("[{mark}] "), Style::default().fg(color)),
            Span::raw(format!("{:<13} {}", c.name, c.detail)),
        ]));
    }
    f.render_widget(
        Paragraph::new(lines)
            .block(block(
                "Doctor: service, Tailscale, multiplexers, permissions",
            ))
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn agents(f: &mut Frame, app: &App, area: Rect) {
    let n = app.agents.len().max(1) as u16 + 2;
    let [top, bottom] = Layout::vertical([
        Constraint::Length(n.min(area.height / 2)),
        Constraint::Min(3),
    ])
    .areas(area);
    let items = app
        .agents
        .iter()
        .map(|a| {
            ListItem::new(format!(
                "{:<10} {:<8} {:<9} {}",
                a.pane, a.agent, a.status, a.title
            ))
        })
        .collect();
    list(f, top, "Agents", items, None);
    let items = app
        .events
        .iter()
        .map(|e| {
            ListItem::new(format!(
                "{:>4} {:<8} {:<17} {:<8} {}",
                e.seq,
                e.agent,
                e.kind,
                if e.pending { "pending" } else { "resolved" },
                e.title
            ))
        })
        .collect();
    list(f, bottom, "Inbox", items, Some(app.selected()));
}

fn devices(f: &mut Frame, app: &App, area: Rect) {
    let items = app
        .devices
        .iter()
        .map(|d| {
            ListItem::new(format!(
                "{:<24} {:<20} {}{}",
                d.id,
                d.name,
                if d.ssh { "ssh" } else { "no ssh key" },
                if d.no_push {
                    "  (no notifications)"
                } else if d.stale {
                    "  (endpoint stale)"
                } else {
                    ""
                }
            ))
        })
        .collect();
    list(f, area, "Paired devices", items, Some(app.selected()));
}

fn pair(f: &mut Frame, app: &App, area: Rect) {
    let text = vec![
        Line::from("Pair a phone by QR over a one-shot, Tailscale-only listener."),
        Line::from(""),
        Line::from("Press Enter: the TUI steps aside, the QR code is shown in this terminal,"),
        Line::from("and you return here when pairing ends (Ctrl+C cancels and burns the token)."),
        Line::from(""),
        Line::from(format!(
            "QR colors: {} (i toggles, for light terminal themes)",
            if app.invert {
                "dark on light"
            } else {
                "light on dark"
            }
        )),
    ];
    f.render_widget(
        Paragraph::new(text)
            .block(block("Pair"))
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn integrations(f: &mut Frame, app: &App, area: Rect) {
    let items = app
        .integrations
        .iter()
        .map(|a| {
            ListItem::new(format!(
                "{:<9} {:<19} {:<10} {:<10} {}",
                a.agent,
                a.name,
                if a.detected { "detected" } else { "not found" },
                match a.installed {
                    Some(true) => "installed",
                    Some(false) => "-",
                    None => "error",
                },
                a.settings
            ))
        })
        .collect();
    list(f, area, "Agent integrations", items, Some(app.selected()));
}

fn setup(f: &mut Frame, app: &App, area: Rect) {
    let mut text = vec![
        Line::from("Setup writes the default config, registers the user service and runs doctor."),
        Line::from("s: keep integrations as they are   a: also install every detected agent"),
        Line::from(""),
    ];
    if let Some(out) = &app.setup_output {
        text.extend(out.lines().map(|l| Line::from(l.to_string())));
    }
    f.render_widget(
        Paragraph::new(text)
            .block(block("Setup"))
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn uninstall(f: &mut Frame, app: &App, area: Rect) {
    let mut text = vec![Line::from(format!(
        "--all (also delete this binary): {}",
        if app.uninstall_all { "yes" } else { "no" }
    ))];
    text.push(Line::from(""));
    let body = app.uninstall_output.as_deref().unwrap_or(&app.plan);
    text.extend(body.lines().map(|l| Line::from(l.to_string())));
    f.render_widget(
        Paragraph::new(text)
            .block(block(if app.uninstall_output.is_some() {
                "Uninstall result"
            } else {
                "Uninstall plan (dry run)"
            }))
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let w = width.min(area.width);
    let h = height.min(area.height);
    Rect::new(
        area.x + (area.width - w) / 2,
        area.y + (area.height - h) / 2,
        w,
        h,
    )
}

fn modal(f: &mut Frame, app: &App) {
    let (title, lines): (&str, Vec<Line>) = match &app.mode {
        Mode::Normal => return,
        Mode::Confirm(p) => (
            "Confirm",
            vec![
                Line::from(p.question()),
                Line::from(""),
                Line::from("y confirm   n / Esc cancel"),
            ],
        ),
        Mode::TypeConfirm { input } => (
            "Confirm uninstall",
            vec![
                Line::from("This removes everything listed in the plan."),
                Line::from(format!("Type \"{UNINSTALL_WORD}\" and press Enter:")),
                Line::from(format!("> {input}")),
                Line::from("Esc cancel"),
            ],
        ),
        Mode::TextInput { title, input, .. } => (
            "Reply text",
            vec![
                Line::from(format!("Reply to \"{title}\":")),
                Line::from(format!("> {input}")),
                Line::from("Enter continue   Esc cancel"),
            ],
        ),
    };
    let area = centered(f.area(), 70, lines.len() as u16 + 2);
    f.render_widget(Clear, area);
    f.render_widget(
        Paragraph::new(lines)
            .block(block(title))
            .wrap(Wrap { trim: false }),
        area,
    );
}
