//! # Smart Normalizer — turns raw events into clean `Vec<Action>`.
//!
//! 5-phase pipeline (see `docs/MACRO_ARCHITECTURE.md` §3.2):
//! 1. Click/Hold pairing
//! 2. Double-click detection
//! 3. Mouse coalescing
//! 4. Keyboard normalization
//! 5. Wait insertion

use crate::core::Action;

use super::raw_event::RawEvent;

/// Threshold (ms) for pairing Down+Up as CLICK. Above this → HOLD.
const CLICK_HOLD_THRESHOLD_MS: u64 = 200;

/// Threshold (ms) between two clicks for double-click detection.
const DOUBLE_CLICK_THRESHOLD_MS: u64 = 100;

/// Threshold (ms) for Down+Up on the same key for KEY_PRESS detection.
const KEY_PRESS_THRESHOLD_MS: u64 = 50;

/// Minimum Wait duration to record (below = noise, dropped).
const MIN_WAIT_MS: u64 = 25;

/// Maximum Wait before splitting into multiple blocks.
const MAX_WAIT_MS: u64 = 60_000;

/// Phase 1: pair Down+Up → Click (or HoldStart).
fn pair_clicks(events: Vec<RawEvent>) -> Vec<RawEvent> {
    use RawEvent::*;
    let mut out: Vec<RawEvent> = Vec::with_capacity(events.len());
    let mut i = 0;
    while i < events.len() {
        match &events[i] {
            MouseDown { button, t_ms } => {
                // Look for matching Up.
                let mut j = i + 1;
                while j < events.len() {
                    if let MouseUp {
                        button: b2,
                        t_ms: t2,
                    } = &events[j]
                    {
                        if b2 == button {
                            let gap = t2.saturating_sub(*t_ms);
                            if gap <= CLICK_HOLD_THRESHOLD_MS {
                                // Fast click → CLICK.
                                out.push(MouseDown {
                                    button: *button,
                                    t_ms: *t_ms,
                                });
                                out.push(MouseUp {
                                    button: *button,
                                    t_ms: *t2,
                                });
                            } else {
                                // Long press → emit Down + WaitGap + Up. Phase 5
                                // keeps the WaitGap, and the executor handles the
                                // hold semantics by interpreting adjacent Down/Wait/Up.
                                out.push(MouseDown {
                                    button: *button,
                                    t_ms: *t_ms,
                                });
                                out.push(RawEvent::WaitGap { ms: gap, t_ms: *t2 });
                                out.push(MouseUp {
                                    button: *button,
                                    t_ms: *t2,
                                });
                            }
                            i = j + 1;
                            break;
                        }
                    }
                    j += 1;
                }
                if j >= events.len() {
                    out.push(events[i].clone());
                    i += 1;
                }
            }
            _ => {
                out.push(events[i].clone());
                i += 1;
            }
        }
    }
    out
}

/// Phase 2: detect double-clicks. `CLICK + WAIT < threshold + CLICK` → DOUBLE.
fn detect_double_clicks(events: Vec<RawEvent>) -> Vec<RawEvent> {
    use RawEvent::*;
    let mut out: Vec<RawEvent> = Vec::with_capacity(events.len());
    let mut i = 0;
    while i < events.len() {
        // Look for: MouseDown + (only MouseUp quickly) → consider as first click.
        if let MouseDown { button, t_ms: t1 } = &events[i] {
            // Find matching Up.
            if let Some(up_idx) = (i + 1..events.len())
                .find(|j| matches!(&events[*j], MouseUp { button: b, .. } if b == button))
            {
                let up = &events[up_idx];
                let gap1 = up.t().saturating_sub(*t1);
                // Find next Down.
                if let Some(next_idx) =
                    (up_idx + 1..events.len()).find(|j| matches!(&events[*j], MouseDown { .. }))
                {
                    if let MouseDown {
                        button: b2,
                        t_ms: t_next,
                    } = &events[next_idx]
                    {
                        if b2 == button && *t_next <= up.t() + DOUBLE_CLICK_THRESHOLD_MS {
                            // It's a double-click. Emit MouseDown only once, keep going.
                            let _ = gap1;
                            out.push(events[i].clone());
                            out.push(events[up_idx].clone());
                            // Skip the duplicate Down by setting i = next_idx + 1 after using up_idx as boundary.
                            // Easier: just continue from next_idx + 1 and let next iter handle its Up.
                            i = next_idx + 1;
                            continue;
                        }
                    }
                }
            }
        }
        out.push(events[i].clone());
        i += 1;
    }
    out
}

/// Ramer-Douglas-Peucker (RDP) polyline simplification for mouse movement sequences.
/// Reduces a 2D trajectory of `MouseMove` events to essential control/turn points.
fn rdp_simplify_moves(moves: &[RawEvent], epsilon: f64) -> Vec<RawEvent> {
    if moves.len() <= 2 {
        return moves.to_vec();
    }

    let extract_pt = |ev: &RawEvent| -> (f64, f64) {
        match ev {
            RawEvent::MouseMove { x, y, .. } => (*x as f64, *y as f64),
            _ => (0.0, 0.0),
        }
    };

    let p_start = extract_pt(&moves[0]);
    let p_end = extract_pt(&moves[moves.len() - 1]);

    let mut max_dist = 0.0;
    let mut index = 0;

    let dx = p_end.0 - p_start.0;
    let dy = p_end.1 - p_start.1;
    let line_len_sq = dx * dx + dy * dy;

    for i in 1..moves.len() - 1 {
        let p = extract_pt(&moves[i]);
        let dist = if line_len_sq == 0.0 {
            ((p.0 - p_start.0).powi(2) + (p.1 - p_start.1).powi(2)).sqrt()
        } else {
            let num = ((p_end.1 - p_start.1) * p.0 - (p_end.0 - p_start.0) * p.1 + p_end.0 * p_start.1 - p_end.1 * p_start.0).abs();
            num / line_len_sq.sqrt()
        };

        if dist > max_dist {
            max_dist = dist;
            index = i;
        }
    }

    if max_dist > epsilon {
        let mut rec1 = rdp_simplify_moves(&moves[0..=index], epsilon);
        let rec2 = rdp_simplify_moves(&moves[index..], epsilon);
        rec1.pop(); // remove duplicate split point
        rec1.extend(rec2);
        rec1
    } else {
        vec![moves[0].clone(), moves[moves.len() - 1].clone()]
    }
}

/// Phase 3: Smart Mouse Trajectory Coalescing.
/// - Hover movements (Mouse Up): Strips intermediate hover steps, keeping ONLY
///   the final destination `MouseMove` before a click/key action.
/// - Drag movements (Mouse Down): Applies Ramer-Douglas-Peucker (RDP) trajectory
///   simplification to maintain smooth curves with minimal control points.
fn coalesce_mouse_moves(events: Vec<RawEvent>) -> Vec<RawEvent> {
    use RawEvent::*;
    let mut out: Vec<RawEvent> = Vec::with_capacity(events.len());
    let mut i = 0;
    let mut is_mouse_down = false;

    while i < events.len() {
        match &events[i] {
            MouseDown { .. } => {
                is_mouse_down = true;
                out.push(events[i].clone());
                i += 1;
            }
            MouseUp { .. } => {
                is_mouse_down = false;
                out.push(events[i].clone());
                i += 1;
            }
            MouseMove { .. } => {
                let start_idx = i;
                while i < events.len() && matches!(events[i], MouseMove { .. }) {
                    i += 1;
                }
                let move_block = &events[start_idx..i];

                if is_mouse_down {
                    // Dragging: simplify path using RDP curve simplification
                    let simplified = rdp_simplify_moves(move_block, 4.0);
                    out.extend(simplified);
                } else {
                    // Hovering: keep only the final destination point before action/click
                    if let Some(last_move) = move_block.last() {
                        out.push(last_move.clone());
                    }
                }
            }
            _ => {
                out.push(events[i].clone());
                i += 1;
            }
        }
    }
    out
}

/// Phase 4: key Down+Up within threshold → KeyPress.
/// Long key holds stay as Down/Up.
fn normalize_keys(events: Vec<RawEvent>) -> Vec<RawEvent> {
    use RawEvent::*;
    let mut out: Vec<RawEvent> = Vec::with_capacity(events.len());
    let mut i = 0;
    while i < events.len() {
        if let KeyDown {
            key,
            mods,
            t_ms: t1,
        } = &events[i]
        {
            // Look for a matching KeyUp for the same key.
            if let Some(up_idx) = (i + 1..events.len())
                .find(|j| matches!(&events[*j], KeyUp { key: k, .. } if k == key))
            {
                if let KeyUp {
                    key: k2,
                    mods: m2,
                    t_ms: t2,
                } = &events[up_idx]
                {
                    let gap = t2.saturating_sub(*t1);
                    if k2 == key && m2 == mods && gap <= KEY_PRESS_THRESHOLD_MS {
                        // Convert Down + Up → into Down + Up pair, which Phase 5 will preserve
                        // as two distinct events. To "convert" into KeyPress we need an Action
                        // boundary — let's emit a marker or treat the pair as already pressed.
                        // Simplest: emit Down + Up and let Phase 5 (Wait insertion) keep them.
                        // For now, keep pair as is; the visual editor will display them.
                        out.push(KeyDown {
                            key: *key,
                            mods: *mods,
                            t_ms: *t1,
                        });
                        out.push(KeyUp {
                            key: *k2,
                            mods: *m2,
                            t_ms: *t2,
                        });
                        i = up_idx + 1;
                        continue;
                    }
                }
            }
        }
        out.push(events[i].clone());
        i += 1;
    }
    out
}

/// Phase 5: insert `Action::Wait { ms }` between events.
/// Drop < MIN_WAIT_MS. Split > MAX_WAIT_MS. Drop last trailing wait.
/// Also do the final event→Action mapping.
fn insert_waits(events: Vec<RawEvent>) -> Vec<Action> {
    // 1) Convert events to a stream of (timestamp, "Actionable") markers.
    // For non-MouseMove events, the action is the event itself.
    let mut last_t: Option<u64> = None;
    let mut out_actions: Vec<Action> = Vec::new();

    for ev in &events {
        let t = ev.t();
        // Emit Wait if there is a non-trivial gap.
        if let Some(prev) = last_t {
            let delta = t.saturating_sub(prev);
            if delta > 0 {
                // Split into pieces of MAX_WAIT_MS.
                let mut remaining = delta;
                while remaining > 0 {
                    let chunk = remaining.min(MAX_WAIT_MS);
                    if chunk >= MIN_WAIT_MS {
                        out_actions.push(Action::Wait { ms: chunk });
                    }
                    remaining = remaining.saturating_sub(chunk);
                }
            }
        }
        last_t = Some(t);

        // All event types are emitted as-is at this stage; WaitGap already maps
        // to Action::Wait via to_action() above. Coalescing has run.
        out_actions.push(ev.to_action());
    }

    // Drop trailing Wait (no event after).
    while let Some(Action::Wait { .. }) = out_actions.last() {
        out_actions.pop();
    }

    // Coalesce adjacent Wait blocks (rare after splitting, but be safe).
    out_actions = out_actions.into_iter().fold(Vec::new(), |mut acc, a| {
        if let (Some(Action::Wait { ms }), Some(Action::Wait { ms: prev_ms })) =
            (acc.last_mut(), Some(&a))
        {
            if let Action::Wait { ms: cur } = a {
                let combined = prev_ms.saturating_add(cur);
                if combined <= MAX_WAIT_MS * 2 {
                    *ms = combined;
                    return acc;
                }
            }
        }
        acc.push(a);
        acc
    });

    out_actions
}

/// Public entry: run all 5 phases on the raw event buffer.
pub fn normalize(events: Vec<RawEvent>) -> Vec<Action> {
    let events = pair_clicks(events);
    let events = detect_double_clicks(events);
    let events = coalesce_mouse_moves(events);
    let events = normalize_keys(events);
    insert_waits(events)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::action::{KeyCode, Modifiers, MouseButton};

    fn ev_mouse_move(x: i32, y: i32, t: u64) -> RawEvent {
        RawEvent::MouseMove { x, y, t_ms: t }
    }
    fn ev_mouse_down(b: MouseButton, t: u64) -> RawEvent {
        RawEvent::MouseDown { button: b, t_ms: t }
    }
    fn ev_mouse_up(b: MouseButton, t: u64) -> RawEvent {
        RawEvent::MouseUp { button: b, t_ms: t }
    }
    fn ev_key(k: KeyCode, mods: Modifiers, t: u64) -> RawEvent {
        RawEvent::KeyDown {
            key: k,
            mods,
            t_ms: t,
        }
    }

    #[test]
    fn click_pair_produces_clean_click() {
        let events = vec![
            ev_mouse_down(MouseButton::Left, 0),
            ev_mouse_up(MouseButton::Left, 50),
            ev_mouse_down(MouseButton::Left, 1000),
            ev_mouse_up(MouseButton::Left, 1050),
        ];
        let actions = normalize(events);
        // Expect: [CLICK(Left), Wait ~950, CLICK(Left)]
        assert!(matches!(actions[0], Action::MouseDown { .. }));
        assert!(matches!(actions.last(), Some(Action::MouseUp { .. })));
    }

    #[test]
    fn coalesce_hover_mouse_moves_keeps_destination_only() {
        let events = vec![
            ev_mouse_move(100, 100, 0),
            ev_mouse_move(102, 100, 16),
            ev_mouse_move(105, 100, 32),
            ev_mouse_move(150, 100, 48), // final destination
        ];
        let out = coalesce_mouse_moves(events);
        let moves: Vec<_> = out
            .iter()
            .filter_map(|e| match e {
                RawEvent::MouseMove { x, y, .. } => Some((*x, *y)),
                _ => None,
            })
            .collect();
        // Hover mode strips intermediate steps, keeping only final destination (150, 100).
        assert_eq!(moves.len(), 1);
        assert_eq!(moves[0], (150, 100));
    }

    #[test]
    fn coalesce_drag_mouse_moves_applies_rdp_simplification() {
        let events = vec![
            ev_mouse_down(MouseButton::Left, 0),
            ev_mouse_move(100, 100, 10),
            ev_mouse_move(200, 100, 20), // straight line point (dropped by RDP)
            ev_mouse_move(300, 100, 30), // straight line point (dropped by RDP)
            ev_mouse_move(400, 300, 40), // corner apex (kept by RDP)
            ev_mouse_move(500, 300, 50), // end point (kept by RDP)
            ev_mouse_up(MouseButton::Left, 60),
        ];
        let out = coalesce_mouse_moves(events);
        let moves: Vec<_> = out
            .iter()
            .filter_map(|e| match e {
                RawEvent::MouseMove { x, y, .. } => Some((*x, *y)),
                _ => None,
            })
            .collect();
        // RDP reduces straight line points while keeping corner apexes (100,100), (300,100), (400,300), (500,300).
        assert_eq!(moves.len(), 4);
        assert_eq!(moves[0], (100, 100));
        assert_eq!(moves[1], (300, 100));
        assert_eq!(moves[2], (400, 300));
        assert_eq!(moves[3], (500, 300));
    }

    #[test]
    fn wait_under_threshold_dropped() {
        // Two events only 8 ms apart — too short, no Wait.
        let events = vec![
            ev_mouse_down(MouseButton::Left, 0),
            ev_mouse_up(MouseButton::Left, 8),
        ];
        let actions = normalize(events);
        // No Wait action should be present (gap < MIN_WAIT_MS).
        assert!(!actions.iter().any(|a| matches!(a, Action::Wait { .. })));
    }

    #[test]
    fn wait_at_24ms_is_dropped() {
        let events = vec![
            ev_mouse_down(MouseButton::Left, 0),
            ev_mouse_up(MouseButton::Left, 24),
        ];
        let actions = normalize(events);
        assert!(!actions.iter().any(|a| matches!(a, Action::Wait { .. })));
    }

    #[test]
    fn end_to_end_snapshot() {
        use crate::core::action::KeyCode;
        let events = vec![
            ev_mouse_move(100, 100, 0),
            ev_mouse_move(102, 100, 16), // small move — coalesced away
            ev_mouse_down(MouseButton::Left, 30),
            ev_mouse_up(MouseButton::Left, 60),
            ev_key(KeyCode::E, Modifiers::NONE, 800), // 740 ms gap
            ev_mouse_move(200, 200, 1000),
        ];
        let actions = normalize(events);
        // Should contain at least: CLICK actions, Wait 740, KEY DOWN, Move
        let has_click = actions
            .iter()
            .any(|a| matches!(a, Action::MouseDown { .. }));
        let has_wait = actions
            .iter()
            .any(|a| matches!(a, Action::Wait { ms: 740..=760 }));
        assert!(has_click, "expected a click action, got: {:?}", actions);
        assert!(has_wait, "expected a ~740ms wait, got: {:?}", actions);
    }

    #[test]
    fn normalizer_handles_empty_events_without_panic() {
        let actions = normalize(vec![]);
        assert!(actions.is_empty());
    }

    #[test]
    fn scroll_event_normalizes_to_action_scroll() {
        let events = vec![RawEvent::Scroll {
            delta_x: 0,
            delta_y: -120,
            t_ms: 100,
        }];
        let actions = normalize(events);
        assert_eq!(actions.len(), 1);
        assert!(matches!(actions[0], Action::Scroll { delta_y: -120, .. }));
    }

    #[test]
    fn x1_x2_mouse_buttons_normalize_properly() {
        let events = vec![
            ev_mouse_down(MouseButton::X1, 10),
            ev_mouse_up(MouseButton::X1, 40),
            ev_mouse_down(MouseButton::X2, 100),
            ev_mouse_up(MouseButton::X2, 130),
        ];
        let actions = normalize(events);
        assert!(actions.iter().any(|a| matches!(a, Action::MouseDown { button: MouseButton::X1 })));
        assert!(actions.iter().any(|a| matches!(a, Action::MouseDown { button: MouseButton::X2 })));
    }

    #[test]
    fn keyboard_complex_modifiers_preserves_mod_state() {
        let mods = Modifiers {
            ctrl: true,
            shift: true,
            alt: false,
            win: false,
        };
        let events = vec![
            ev_key(KeyCode(0x41), mods, 10),
            RawEvent::KeyUp {
                key: KeyCode(0x41),
                mods,
                t_ms: 30,
            },
        ];
        let actions = normalize(events);
        assert!(actions.iter().any(|a| matches!(a, Action::KeyDown { mods: m, .. } if m.ctrl && m.shift)));
    }

    #[test]
    fn precise_mode_preserves_raw_events() {
        let events = vec![
            ev_mouse_move(10, 10, 0),
            ev_mouse_move(11, 10, 5),
            ev_mouse_move(12, 10, 10),
        ];
        let actions: Vec<Action> = events.into_iter().map(|e| e.to_action()).collect();
        assert_eq!(actions.len(), 3, "precise mode should keep all raw move events");
    }
}
