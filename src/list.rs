use std::time::Duration;

use anyhow::Result;

use crate::protocol::{Request, SessionView};
use crate::sock;
use crate::tui::layout::{pad_to_width, truncate_to_width};

/// Run roost list: connect to daemon, get session views, print formatted table.
pub fn run_list() -> Result<()> {
    let path = sock::socket_path();

    if !sock::is_listening(&path) {
        println!("No roost daemon is running. Start it with: roost daemon");
        println!("Or use 'roost setup' to install hooks and then start Claude Code.");
        return Ok(());
    }

    let views = query_daemon()?;

    if views.is_empty() {
        println!("No agents are currently running.");
        println!("roost watches your Claude Code & Codex sessions…");
        return Ok(());
    }

    print_sessions(&views);
    Ok(())
}

/// Query daemon for current session list.
pub fn query_daemon() -> Result<Vec<SessionView>> {
    let path = sock::socket_path();
    let mut stream = sock::connect_with_timeout(&path, Duration::from_secs(2))?;

    let request = Request::List;
    let json = serde_json::to_string(&request)?;
    sock::send_line(&mut stream, &json)?;

    let response = sock::recv_line(&mut stream)?;
    if response.is_empty() {
        return Ok(vec![]);
    }

    let views: Vec<SessionView> = serde_json::from_str(&response)?;
    Ok(views)
}

/// Column widths (in display columns, CJK-aware).
const COL_ICON: usize = 5; // icon + space, left-padded to fixed width
const COL_NAME: usize = 16;
const COL_STATE: usize = 10;
const COL_ACTIVITY: usize = 30;
const COL_TIME: usize = 4;

/// Print session views as a formatted table.
pub fn print_sessions(views: &[SessionView]) {
    // Header: use plain ASCII so column widths align with data rows
    let header_name = pad_to_width("NAME", COL_NAME);
    let header_state = pad_to_width("STATE", COL_STATE);
    let header_activity = pad_to_width("ACTIVITY", COL_ACTIVITY);
    println!(
        "{} {} {} {} AGO",
        pad_to_width("AGENT", COL_ICON),
        header_name,
        header_state,
        header_activity
    );
    println!(
        "{}",
        "-".repeat(COL_ICON + 1 + COL_NAME + 1 + COL_STATE + 1 + COL_ACTIVITY + 1 + COL_TIME)
    );

    for v in views {
        let family_icon = match v.family {
            crate::protocol::AgentFamily::Claude => "✻",
            crate::protocol::AgentFamily::Codex => "⬡",
            crate::protocol::AgentFamily::Unknown => "◆",
        };

        let activity = v.activity.as_deref().unwrap_or(
            // When idle/done, show "(idle)" or "(done)"
            if v.state == "done" { "(done)" } else { "" },
        );

        let time_str = format_relative_time(v.last_activity_secs);

        // Use display-width-aware truncation + padding so CJK chars align.
        let name_cell = pad_to_width(&truncate_to_width(&v.name, COL_NAME), COL_NAME);
        let state_cell = pad_to_width(&truncate_to_width(&v.state, COL_STATE), COL_STATE);
        let activity_cell = pad_to_width(&truncate_to_width(activity, COL_ACTIVITY), COL_ACTIVITY);
        let icon_cell = pad_to_width(family_icon, COL_ICON);

        println!(
            "{} {} {} {} {}",
            icon_cell, name_cell, state_cell, activity_cell, time_str
        );
    }
}

fn format_relative_time(secs: u64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else {
        format!("{}h", secs / 3600)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::AgentFamily;

    #[test]
    fn print_sessions_does_not_panic() {
        let views = vec![
            SessionView {
                id: "s1".to_string(),
                family: AgentFamily::Claude,
                name: "my-repo".to_string(),
                state: "working".to_string(),
                activity: Some("思考中…".to_string()),
                last_activity_secs: 5,
                busy_secs: 0,
                first_prompt: None,
                host_app: None,
                terminal_session_id: None,
                terminal_tty: None,
                tmux_target: None,
                wezterm_pane: None,
                zellij_session: None,
                workspace_path: None,
                codex_thread_id: None,
                pane_title: None,
            },
            SessionView {
                id: "s2".to_string(),
                family: AgentFamily::Codex,
                name: "another-repo".to_string(),
                state: "done".to_string(),
                activity: None,
                last_activity_secs: 120,
                busy_secs: 0,
                first_prompt: None,
                host_app: None,
                terminal_session_id: None,
                terminal_tty: None,
                tmux_target: None,
                wezterm_pane: None,
                zellij_session: None,
                workspace_path: None,
                codex_thread_id: None,
                pane_title: None,
            },
        ];
        print_sessions(&views);
    }

    #[test]
    fn format_relative_time_cases() {
        assert_eq!(format_relative_time(0), "0s");
        assert_eq!(format_relative_time(59), "59s");
        assert_eq!(format_relative_time(60), "1m");
        assert_eq!(format_relative_time(3599), "59m");
        assert_eq!(format_relative_time(3600), "1h");
    }

    /// CJK names and activity text must align correctly in the table.
    /// Each CJK character is 2 display columns wide; the cells must still be
    /// padded to the same total column width so borders line up.
    #[test]
    fn cjk_name_and_activity_cells_have_correct_display_width() {
        use unicode_width::UnicodeWidthStr;

        let cjk_name = "支付服务接口"; // 6 CJK chars = 12 display cols
        let cjk_activity = "思考中…"; // 4 chars: 3×CJK(6) + ellipsis(1) = 7 display cols

        let name_cell = pad_to_width(&truncate_to_width(cjk_name, COL_NAME), COL_NAME);
        let activity_cell =
            pad_to_width(&truncate_to_width(cjk_activity, COL_ACTIVITY), COL_ACTIVITY);

        assert_eq!(
            UnicodeWidthStr::width(name_cell.as_str()),
            COL_NAME,
            "name cell display width should be {COL_NAME}, got {}",
            UnicodeWidthStr::width(name_cell.as_str())
        );
        assert_eq!(
            UnicodeWidthStr::width(activity_cell.as_str()),
            COL_ACTIVITY,
            "activity cell display width should be {COL_ACTIVITY}, got {}",
            UnicodeWidthStr::width(activity_cell.as_str())
        );
    }
}
