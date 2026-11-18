//! Rendering of the TUI state. No I/O: draws into a ratatui `Frame`.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Frame;

use super::actions;
use super::i18n::tf;
use super::state::{
    fix_title, App, HomeItem, Input, Mode, Screen, StepStatus, UpdateState, WizardStep,
    UNINSTALL_WORD,
};

const REVERSED: Style = Style::new().add_modifier(Modifier::REVERSED);

/// Greedy word wrap of `items` (each kept whole, `sep` between them) into
/// lines of `width`.
fn wrap_items<'a>(items: Vec<Span<'a>>, sep: &'static str, width: usize) -> Vec<Line<'a>> {
    let mut lines: Vec<Line> = Vec::new();
    let mut cur: Vec<Span> = Vec::new();
    let mut used = 0;
    for s in items {
        let w = s.content.chars().count();
        if used > 0 && used + sep.len() + w > width {
            lines.push(Line::from(std::mem::take(&mut cur)));
            used = 0;
        }
        if used > 0 {
            cur.push(Span::raw(sep));
            used += sep.len();
        }
        used += w;
        cur.push(s);
    }
    if !cur.is_empty() {
        lines.push(Line::from(cur));
    }
    lines
}

fn footer_lines(app: &App, width: usize) -> Vec<Line<'static>> {
    let msg = app.message.as_ref().map_or(Line::from(""), |m| {
        Line::from(Span::styled(
            m.text.clone(),
            Style::default().fg(if m.error { Color::Red } else { Color::Green }),
        ))
    });
    let tabs = Screen::ALL
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let label = format!("{} {}", i + 1, app.tr(s.tab()));
            if *s == app.screen {
                Span::styled(label, REVERSED.add_modifier(Modifier::BOLD))
            } else {
                Span::raw(label)
            }
        })
        .collect();
    let mut hints: Vec<Span> = if app.wizard.is_some() {
        [("Enter", "run"), ("s", "skip"), ("Esc", "leave")]
            .iter()
            .map(|(k, h)| Span::raw(format!("{k} {}", app.tr(h))))
            .collect()
    } else {
        actions::on(app.screen)
            .filter(|d| !d.key.is_empty())
            .map(|d| Span::raw(format!("{} {}", d.key, app.tr(d.hint))))
            .collect()
    };
    for (k, h) in [("r", "refresh"), ("q", "quit")] {
        hints.push(Span::raw(format!("{k} {}", app.tr(h))));
    }
    let mut lines = vec![msg];
    lines.extend(wrap_items(tabs, " ", width));
    lines.extend(wrap_items(hints, "  ", width));
    lines
}

pub fn draw(f: &mut Frame, app: &App) {
    let area = f.area();
    let footer = footer_lines(app, area.width.max(1) as usize);
    // The body keeps at least three rows; the footer gets the rest.
    let fh = (footer.len() as u16 + 1).min(area.height.saturating_sub(3));
    let [body, foot] = Layout::vertical([Constraint::Min(0), Constraint::Length(fh)]).areas(area);

    let title = if app.wizard.is_some() {
        app.tr("Setup wizard")
    } else {
        app.tr(app.screen.title())
    };
    let outer = Block::default()
        .borders(Borders::ALL)
        .title(format!(" Falconer ─ {title} "));
    let inner = outer.inner(body);
    f.render_widget(outer, body);

    if app.wizard.is_some() {
        wizard(f, app, inner);
    } else {
        screen(f, app, inner);
    }

    f.render_widget(
        Paragraph::new(footer).block(Block::default().borders(Borders::TOP)),
        foot,
    );

    modal(f, app);
}

fn screen(f: &mut Frame, app: &App, inner: Rect) {
    match app.screen {
        Screen::Home => home(f, app, inner),
        Screen::Agents => agents(f, app, inner),
        Screen::Phones => phones(f, app, inner),
        Screen::Integrations => integrations(f, app, inner),
        Screen::Service => service(f, app, inner),
        Screen::Settings => settings(f, app, inner),
        Screen::Usage => usage(f, app, inner),
        Screen::Advanced => advanced(f, app, inner),
    }
}

fn wizard(f: &mut Frame, app: &App, area: Rect) {
    let Some(cur) = app.wizard else { return };
    let step = WizardStep::ALL[cur];
    let [top, bottom] = Layout::vertical([
        Constraint::Length(WizardStep::ALL.len() as u16 + 1),
        Constraint::Min(0),
    ])
    .areas(area);
    let items = WizardStep::ALL
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let (m, color, note) = match app.step_status(*s) {
                StepStatus::Done => ("✓", Color::Green, app.tr("done")),
                StepStatus::Unknown => ("?", Color::Yellow, app.tr("checking")),
                StepStatus::Todo => ("○", Color::Red, app.tr("to do")),
            };
            ListItem::new(Line::from(vec![
                Span::styled(format!("{m} "), Style::default().fg(color)),
                Span::raw(format!("{} {:<30} {note}", i + 1, app.tr(s.title()))),
            ]))
        })
        .collect();
    let title = tf(
        app.lang,
        "Step {}/{}",
        &[&(cur + 1).to_string(), &WizardStep::ALL.len().to_string()],
    );
    list(f, app, top, &title, items, Some(cur));
    let next = match (app.step_status(step), step, app.step_fix(step)) {
        (StepStatus::Done, ..) => app.tr("Done. Enter continues.").to_string(),
        (StepStatus::Unknown, ..) => app.tr("Status unknown. Enter checks again.").to_string(),
        (StepStatus::Todo, WizardStep::Integrations, _) => {
            app.tr("Enter installs them, after asking.").to_string()
        }
        (StepStatus::Todo, _, Some(id)) => tf(
            app.lang,
            "Enter: {}, after asking.",
            &[fix_title(app.lang, id)],
        ),
        (StepStatus::Todo, _, None) => String::new(),
    };
    text_panel(
        f,
        bottom,
        app.tr(step.title()),
        vec![
            Line::from(app.tr(step.about())),
            Line::from(""),
            Line::from(next),
            Line::from(app.tr("s skips this step; Esc leaves the wizard for Home.")),
        ],
    );
}

/// A section inside the screen: a titled rule above its content.
fn block(title: &str) -> Block<'_> {
    Block::default()
        .borders(Borders::TOP)
        .title(format!(" {title} "))
}

fn list(
    f: &mut Frame,
    app: &App,
    area: Rect,
    title: &str,
    items: Vec<ListItem>,
    sel: Option<usize>,
) {
    let empty = items.is_empty();
    let items = if empty {
        vec![ListItem::new(app.tr("(none)"))]
    } else {
        items
    };
    let mut state = ListState::default().with_selected(if empty { None } else { sel });
    f.render_stateful_widget(
        List::new(items)
            .block(block(title))
            .highlight_symbol("› ")
            .highlight_style(REVERSED),
        area,
        &mut state,
    );
}

fn text_panel(f: &mut Frame, area: Rect, title: &str, lines: Vec<Line>) {
    f.render_widget(
        Paragraph::new(lines)
            .block(block(title))
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn lines_of(s: &str) -> Vec<Line<'static>> {
    s.lines().map(|l| Line::from(l.to_string())).collect()
}

fn mark(status: &str) -> (&'static str, Color) {
    match status {
        "ok" => ("✓", Color::Green),
        "warn" => ("!", Color::Yellow),
        _ => ("✗", Color::Red),
    }
}

fn heading(s: &str) -> ListItem<'static> {
    ListItem::new(Line::from(Span::styled(
        s.to_string(),
        Style::default().add_modifier(Modifier::BOLD),
    )))
}

fn home(f: &mut Frame, app: &App, area: Rect) {
    let lang = app.lang;
    let health = match app.healthy {
        Some(true) => app.tr("all good"),
        Some(false) => app.tr("needs attention"),
        None => app.tr("unknown"),
    };
    let update = match &app.update {
        UpdateState::Unknown => String::new(),
        UpdateState::Checking => app.tr("Checking for updates…").to_string(),
        UpdateState::UpToDate => app.tr("Up to date").to_string(),
        UpdateState::Available { latest, .. } => tf(lang, "v{} available", &[latest]),
        UpdateState::Failed => app.tr("Update check failed").to_string(),
    };
    let mut items = vec![ListItem::new(format!(
        "{} {} · {} {health}{}",
        app.tr("Version"),
        app.version.as_deref().unwrap_or("?"),
        app.tr("Health:"),
        if update.is_empty() {
            String::new()
        } else {
            format!(" · {update}")
        }
    ))];
    let mut selectable: Vec<usize> = Vec::new();
    items.push(heading(app.tr("Health")));
    let home_items = app.home_items();
    let mut quick_heading = false;
    for item in &home_items {
        let label = match item {
            HomeItem::Check(i) => {
                let c = &app.checks[*i];
                let (m, color) = mark(&c.status);
                let mut lines = vec![Line::from(vec![
                    Span::styled(format!("{m} "), Style::default().fg(color)),
                    Span::raw(format!("{:<12} {}", c.name, c.detail)),
                ])];
                lines.push(Line::from(match c.fix {
                    Some(id) => format!("    → {}", fix_title(lang, id)),
                    None => format!("    {}", app.tr("(no automatic fix)")),
                }));
                selectable.push(items.len());
                items.push(ListItem::new(lines));
                continue;
            }
            HomeItem::FixAll => app.tr("Install what's missing").to_string(),
            HomeItem::Pair => app.tr("Pair a phone").to_string(),
            HomeItem::Update => match &app.update {
                UpdateState::Available { latest, .. } => {
                    tf(lang, "Update Falconer (v{} available)", &[latest])
                }
                _ => continue,
            },
            HomeItem::Wizard => app.tr("Run setup wizard").to_string(),
        };
        if !quick_heading {
            let ok: Vec<&str> = app
                .checks
                .iter()
                .filter(|c| c.ok())
                .map(|c| c.name.as_str())
                .collect();
            if !ok.is_empty() {
                items.push(ListItem::new(Line::from(vec![
                    Span::styled("✓ ", Style::default().fg(Color::Green)),
                    Span::raw(tf(
                        lang,
                        "{} OK: {}",
                        &[&ok.len().to_string(), &ok.join(", ")],
                    )),
                ])));
            }
            items.push(heading(app.tr("Quick actions")));
            quick_heading = true;
        }
        selectable.push(items.len());
        items.push(ListItem::new(label));
    }
    let sel = selectable.get(app.selected()).copied();
    let mut state = ListState::default().with_selected(sel);
    f.render_stateful_widget(
        List::new(items)
            .highlight_symbol("› ")
            .highlight_style(REVERSED),
        area,
        &mut state,
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
    list(f, app, top, app.tr("Agents"), items, None);
    let items = app
        .events
        .iter()
        .map(|e| {
            ListItem::new(format!(
                "{:>4} {:<8} {:<17} {:<8} {}",
                e.seq,
                e.agent,
                e.kind,
                if e.pending {
                    app.tr("pending")
                } else {
                    app.tr("resolved")
                },
                e.title
            ))
        })
        .collect();
    list(f, app, bottom, app.tr("Inbox"), items, Some(app.selected()));
}

fn phones(f: &mut Frame, app: &App, area: Rect) {
    let [top, bottom] = Layout::vertical([Constraint::Min(3), Constraint::Length(5)]).areas(area);
    let items = app
        .devices
        .iter()
        .map(|d| {
            ListItem::new(format!(
                "{:<24} {:<20} {}{}{}",
                d.id,
                d.name,
                if d.ssh {
                    app.tr("ssh")
                } else {
                    app.tr("no ssh key")
                },
                if d.no_push {
                    app.tr("  (no notifications)")
                } else if d.stale {
                    app.tr("  (endpoint stale)")
                } else {
                    ""
                },
                if d.webpush { "  [web push]" } else { "" }
            ))
        })
        .collect();
    list(
        f,
        app,
        top,
        app.tr("Paired phones"),
        items,
        Some(app.selected()),
    );
    text_panel(
        f,
        bottom,
        app.tr("Pair"),
        vec![
            Line::from(app.tr(
                "p pairs a phone by QR over a one-shot, Tailscale-only listener in this terminal.",
            )),
            Line::from(tf(
                app.lang,
                "QR colors: {} (i inverts, for light terminal themes)",
                &[if app.invert {
                    app.tr("dark on light")
                } else {
                    app.tr("light on dark")
                }],
            )),
        ],
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
                if a.detected {
                    app.tr("detected")
                } else {
                    app.tr("not found")
                },
                match a.installed {
                    Some(true) => app.tr("installed"),
                    Some(false) => "-",
                    None => app.tr("error"),
                },
                a.settings
            ))
        })
        .collect();
    list(
        f,
        app,
        area,
        app.tr("Agent integrations"),
        items,
        Some(app.selected()),
    );
}

fn service(f: &mut Frame, app: &App, area: Rect) {
    let mut text = Vec::new();
    match app.checks.iter().find(|c| c.name == "service") {
        Some(c) => {
            let (m, color) = mark(&c.status);
            text.push(Line::from(vec![
                Span::styled(format!("{m} "), Style::default().fg(color)),
                Span::raw(c.detail.clone()),
            ]));
        }
        None => text.push(Line::from(app.tr("Service status unknown; r refreshes."))),
    }
    text.push(Line::from(""));
    text.push(Line::from(app.tr(
        "The user service runs `falconer-hook serve`: it watches agents and sends notifications.",
    )));
    if let Some(out) = &app.service_output {
        text.push(Line::from(""));
        text.extend(lines_of(out));
    }
    text_panel(f, area, app.tr("User service"), text);
}

fn settings(f: &mut Frame, app: &App, area: Rect) {
    let n = app.devices.len().max(1) as u16 + 2;
    let [top, bottom] = Layout::vertical([
        Constraint::Length(n.min(area.height / 2)),
        Constraint::Min(3),
    ])
    .areas(area);
    let items = app
        .devices
        .iter()
        .map(|d| {
            ListItem::new(format!(
                "{:<20} {}",
                d.name,
                if d.endpoint.is_empty() {
                    app.tr("(no push endpoint)")
                } else {
                    d.endpoint.as_str()
                }
            ))
        })
        .collect();
    list(
        f,
        app,
        top,
        app.tr("Push endpoint per phone"),
        items,
        Some(app.selected()),
    );
    text_panel(
        f,
        bottom,
        app.tr("Config (read-only)"),
        lines_of(&app.config_text),
    );
}

fn usage(f: &mut Frame, app: &App, area: Rect) {
    let days = app.usage_days.to_string();
    let mut text = vec![
        Line::from(tf(
            app.lang,
            "Last {} days, from the agents' own local logs (+/- change the range).",
            &[&days],
        )),
        Line::from(""),
    ];
    text.extend(lines_of(&app.usage_text));
    text_panel(f, area, app.tr("Token usage"), text);
}

fn advanced(f: &mut Frame, app: &App, area: Rect) {
    let acts = app.advanced_items();
    let [top, bottom] = Layout::vertical([
        Constraint::Length((acts.len() as u16 + 2).min(area.height / 2)),
        Constraint::Min(3),
    ])
    .areas(area);
    let items = acts
        .iter()
        .map(|a| ListItem::new(app.tr(actions::def(*a).label)))
        .collect();
    list(f, app, top, app.tr("Actions"), items, Some(app.selected()));
    let (title, body) = match &app.output {
        Some(out) => (app.tr("Output").to_string(), out.as_str()),
        None => (
            tf(
                app.lang,
                "Uninstall plan (dry run), --all: {}",
                &[if app.uninstall_all {
                    app.tr("yes")
                } else {
                    app.tr("no")
                }],
            ),
            app.plan.as_str(),
        ),
    };
    text_panel(f, bottom, &title, lines_of(body));
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

/// Draws a centered dialog that fits `lines` (wrapped) when it can.
pub fn dialog(f: &mut Frame, title: &str, lines: Vec<Line>) {
    let area = f.area();
    let width = 76.min(area.width);
    let inner = width.saturating_sub(2).max(1) as usize;
    let rows: usize = lines.iter().map(|l| l.width().max(1).div_ceil(inner)).sum();
    let area = centered(area, width, rows as u16 + 2);
    f.render_widget(Clear, area);
    f.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(format!(" {title} ")),
            )
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn modal(f: &mut Frame, app: &App) {
    let (title, lines): (&str, Vec<Line>) = match &app.mode {
        Mode::Normal => return,
        Mode::Confirm(p) => {
            let mut l = lines_of(&p.question(app.lang));
            l.push(Line::from(""));
            l.push(Line::from(app.tr("y confirm   n / Esc cancel")));
            (app.tr("Confirm"), l)
        }
        Mode::TypeConfirm { input } => (
            app.tr("Confirm uninstall"),
            vec![
                Line::from(app.tr("This removes everything listed in the plan.")),
                Line::from(tf(
                    app.lang,
                    "Type \"{}\" and press Enter:",
                    &[UNINSTALL_WORD],
                )),
                Line::from(format!("> {input}")),
                Line::from(app.tr("Esc cancel")),
            ],
        ),
        Mode::TextInput { purpose, input } => {
            let (title, prompt) = match purpose {
                Input::Reply { title, .. } => (
                    app.tr("Reply text"),
                    tf(app.lang, "Reply to \"{}\":", &[title]),
                ),
                Input::Webpush { name, .. } => (
                    app.tr("Web Push subscription"),
                    tf(
                        app.lang,
                        "Paste the subscription JSON for {} (as for devices set-webpush):",
                        &[name],
                    ),
                ),
                Input::Endpoint { name, .. } => (
                    app.tr("Push endpoint"),
                    tf(app.lang, "New push endpoint URL for {}:", &[name]),
                ),
            };
            (
                title,
                vec![
                    Line::from(prompt),
                    Line::from(format!("> {input}")),
                    Line::from(app.tr("Enter continue   Esc cancel")),
                ],
            )
        }
    };
    dialog(f, title, lines);
}
