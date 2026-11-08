//! Where a download's server is (#140): one line, shared by Download File Info, the progress
//! window and Properties. The country's flag and name (`mdm_geo`: DB-IP's offline table, looked up
//! on this computer, never asked of anyone), and the round trip the download's own connections
//! measured (`mdm_engine::Server`), coloured near / medium / far with the word beside it. Its
//! tooltip: the address, a CDN or a proxy said as such, how the time is measured, and DB-IP's
//! credit.

use std::net::IpAddr;
use std::sync::OnceLock;

use egui::{Color32, Rect, Sense, Ui, pos2, vec2};
use serde::{Deserialize, Serialize};

use crate::winui;

/// What a download learnt of its server, kept with it (Properties shows it after it ends).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Seen {
    #[serde(default)]
    pub ip: Option<IpAddr>,
    #[serde(default)]
    pub rtt_ms: Option<u32>,
    #[serde(default)]
    pub proxied: bool,
    #[serde(default)]
    pub cdn: Option<String>,
}

impl From<&mdm_engine::Server> for Seen {
    fn from(s: &mdm_engine::Server) -> Self {
        Seen {
            ip: s.ip,
            rtt_ms: s.rtt.map(|d| d.as_millis().min(u32::MAX as u128) as u32),
            proxied: s.proxied,
            cdn: s.cdn.map(str::to_owned),
        }
    }
}

/// How far the server is, by its round trip.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reach {
    Near,
    Medium,
    Far,
}

/// Under 60 ms near (the same country or next door), under 150 ms medium (the same continent),
/// else far.
pub fn reach(ms: u32) -> Reach {
    match ms {
        0..60 => Reach::Near,
        60..150 => Reach::Medium,
        _ => Reach::Far,
    }
}

fn reach_word(r: Reach) -> &'static str {
    match r {
        // i18n: how far a download's server is, by its round trip (under 60 ms)
        Reach::Near => crate::i18n::t("near"),
        // i18n: how far a download's server is, by its round trip (60 to 150 ms)
        Reach::Medium => crate::i18n::t("medium"),
        // i18n: how far a download's server is, by its round trip (over 150 ms)
        Reach::Far => crate::i18n::t("far"),
    }
}

fn reach_colour(r: Reach) -> Color32 {
    let c = crate::theme::colors();
    match r {
        Reach::Near => c.reach_near,
        Reach::Medium => c.reach_medium,
        Reach::Far => c.reach_far,
    }
}

/// The flags' picture, once.
fn flags(ctx: &egui::Context) -> Option<egui::TextureHandle> {
    static FLAGS: OnceLock<Option<egui::TextureHandle>> = OnceLock::new();
    FLAGS
        .get_or_init(|| {
            let img = image::load_from_memory(mdm_geo::FLAGS_PNG).ok()?.to_rgba8();
            let size = [img.width() as usize, img.height() as usize];
            let ci = egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw());
            Some(ctx.load_texture("country-flags", ci, egui::TextureOptions::LINEAR))
        })
        .clone()
}

/// This computer's own addresses (loopback): its icon is a computer, not the local network's.
fn is_this_computer(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(a) => a.is_loopback(),
        IpAddr::V6(a) => a.is_loopback() || a.to_ipv4_mapped().is_some_and(|v4| v4.is_loopback()),
    }
}

fn is_local(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(a) => a.is_private() || a.is_loopback() || a.is_link_local(),
        IpAddr::V6(a) => {
            let s = a.segments()[0];
            a.is_loopback() || (s & 0xfe00) == 0xfc00 || (s & 0xffc0) == 0xfe80
        }
    }
}

/// Where a place's picture comes from: its country's flag, or a Tango icon for this computer
/// and the local network, which have no country.
enum Mark {
    Flag(mdm_geo::Country),
    Icon(&'static str),
}

/// The place's words and its mark (a flag or an icon), or None while the table loads.
fn place(seen: &Seen) -> Option<(String, Option<Mark>)> {
    let Some(ip) = seen.ip else {
        // i18n: where a download's server is: not known (no address seen)
        return Some((crate::i18n::t("unknown").to_owned(), None));
    };
    if !mdm_geo::ready() {
        mdm_geo::prepare();
        return None;
    }
    let country = mdm_geo::country(ip);
    let (name, icon) = match &country {
        Some(c) => (
            crate::countries::name(c.code())
                .unwrap_or(c.code())
                .to_owned(),
            None,
        ),
        // i18n: where a download's server is: on this very computer (no country)
        None if is_this_computer(ip) => {
            (crate::i18n::t("this computer").to_owned(), Some("computer"))
        }
        // i18n: where a download's server is: another computer on the local network (no country)
        None if is_local(ip) => (
            crate::i18n::t("the local network").to_owned(),
            Some("network-workgroup"),
        ),
        // i18n: where a download's server is: an address the location table doesn't place
        None => (crate::i18n::t("unknown").to_owned(), None),
    };
    let words = if seen.proxied {
        match &country {
            Some(_) => crate::i18n::fmt(
                // i18n: where a download's server is, when a proxy carries it: {country} is the proxy's country
                crate::i18n::t("through a proxy in {country}"),
                &[("country", &name)],
            ),
            // i18n: where a download's server is, when a proxy carries it (the proxy's place not known)
            None => crate::i18n::t("through a proxy").to_owned(),
        }
    } else {
        name
    };
    let mark = country.map(Mark::Flag).or(icon.map(Mark::Icon));
    Some((words, mark))
}

/// What the line's tooltip says.
fn tooltip(seen: &Seen) -> String {
    let mut lines = Vec::new();
    if let Some(ip) = seen.ip {
        lines.push(crate::i18n::fmt(
            // i18n: the tooltip of where a download's server is: its IP address
            crate::i18n::t("Address: {address}"),
            &[("address", &crate::i18n::ltr(&ip.to_string()))],
        ));
    }
    if let Some(cdn) = &seen.cdn {
        lines.push(crate::i18n::fmt(
            // i18n: the tooltip of where a download's server is: {cdn} is a content delivery network's name (Cloudflare…)
            crate::i18n::t(
                "{cdn} serves this file: the place is its server's, often near you, not the site owner's.",
            ),
            &[("cdn", cdn)],
        ));
    }
    if seen.proxied {
        lines.push(
            // i18n: the tooltip of where a download's server is, when a proxy carries the download
            crate::i18n::t(
                "A proxy carries this download: the place and the time are the proxy's, not the server's.",
            )
            .to_owned(),
        );
    }
    if seen.rtt_ms.is_some() {
        lines.push(
            // i18n: the tooltip of where a download's server is: how the time shown is measured
            crate::i18n::t(
                "The time is a round trip, measured as this download's own connections opened.",
            )
            .to_owned(),
        );
    }
    // The data's licence (CC BY 4.0) asks for this credit where its results show.
    lines.push(format!(
        "{} ({})",
        mdm_geo::ATTRIBUTION,
        mdm_geo::ATTRIBUTION_URL.trim_start_matches("https://")
    ));
    lines.join("\n")
}

/// The line, its start at English `x` and its baseline at `baseline`, never past English
/// `right`; "..." while nothing is known yet. `id` tells this line from others.
pub fn line(ui: &Ui, x: f32, baseline: f32, right: f32, seen: Option<&Seen>, id: &str) {
    let p = ui.painter().clone();
    let Some(seen) = seen else {
        winui::text(&p, x, baseline, "...", winui::MUTED());
        return;
    };
    let Some((words, mark)) = place(seen) else {
        winui::text(&p, x, baseline, "...", winui::MUTED());
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(100));
        return;
    };
    let mut at = x;
    let mut shown: Option<Rect> = None;
    // This computer's or the local network's icon, 16 x 16, in the flag's place: its drawing's
    // middle level with the middle of the text's letters (measured on the pictures).
    if let Some(Mark::Icon(name)) = &mark {
        let rc = winui::rect(Rect::from_min_size(
            pos2(at, baseline - 11.0),
            vec2(16.0, 16.0),
        ));
        crate::fileicons::paint_tango(ui, &p, name, rc);
        shown = Some(rc);
        at += 21.0;
    }
    // The flag, 16 x 12, its middle on the text's.
    let country = match &mark {
        Some(Mark::Flag(c)) => Some(c),
        _ => None,
    };
    if let Some((cx, cy)) = country.and_then(|c| c.flag_cell())
        && let Some(tex) = flags(ui.ctx())
    {
        let size = tex.size_vec2();
        let uv = Rect::from_min_size(
            pos2(cx as f32 / size.x, cy as f32 / size.y),
            vec2(
                mdm_geo::FLAG_W as f32 / size.x,
                mdm_geo::FLAG_H as f32 / size.y,
            ),
        );
        let rc = winui::rect(Rect::from_min_size(
            pos2(at, baseline - 10.0),
            vec2(16.0, 12.0),
        ));
        p.image(tex.id(), rc, uv, Color32::WHITE);
        p.rect_stroke(
            rc,
            0.0,
            egui::Stroke::new(1.0, crate::theme::colors().flag_edge),
            egui::StrokeKind::Inside,
        );
        shown = Some(rc);
        at += 21.0;
    }
    let time = seen.rtt_ms.map(|ms| {
        let r = reach(ms);
        (
            crate::i18n::fmt(
                // i18n: a download's server's round trip: {ms} a number of milliseconds, {reach} near, medium or far
                crate::i18n::t("about {ms} ms ({reach})"),
                // Under a millisecond (this computer, the local network) says "about 1 ms".
                &[("ms", &ms.max(1).to_string()), ("reach", reach_word(r))],
            ),
            reach_colour(r),
        )
    });
    // The time keeps its room; the place is cut first.
    let time_w = time
        .as_ref()
        .map_or(0.0, |(t, _)| winui::text_width(&p, t) + 12.0);
    let (rc, cut) = winui::text_fit(
        &p,
        at,
        baseline,
        &words,
        winui::TEXT(),
        (right - time_w).max(at + 30.0),
    );
    let mut all = shown.map_or(rc, |s| s.union(rc));
    if let Some((t, colour)) = time {
        let after = at + rc.width() + 6.0;
        let dot = winui::separator_dot(&p, after, baseline, winui::MUTED());
        let rc = winui::text(&p, after + dot.width() + 5.0, baseline, &t, colour);
        all = all.union(dot).union(rc);
    }
    let full = if cut {
        format!("{words}\n{}", tooltip(seen))
    } else {
        tooltip(seen)
    };
    ui.interact(all, egui::Id::unique(("server-line", id)), Sense::hover())
        .on_hover_text(full);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_are_near_medium_or_far() {
        assert_eq!(reach(5), Reach::Near);
        assert_eq!(reach(59), Reach::Near);
        assert_eq!(reach(60), Reach::Medium);
        assert_eq!(reach(149), Reach::Medium);
        assert_eq!(reach(150), Reach::Far);
        assert_eq!(reach(900), Reach::Far);
    }

    #[test]
    fn what_was_seen_is_kept_in_milliseconds() {
        let s = mdm_engine::Server {
            ip: Some("8.8.8.8".parse().unwrap()),
            rtt: Some(std::time::Duration::from_micros(23_400)),
            proxied: false,
            cdn: Some("Cloudflare"),
        };
        let seen = Seen::from(&s);
        assert_eq!(seen.rtt_ms, Some(23));
        assert_eq!(seen.cdn.as_deref(), Some("Cloudflare"));
        let back: Seen = serde_json::from_str(&serde_json::to_string(&seen).unwrap()).unwrap();
        assert_eq!(back, seen);
        // Kept from before: nothing.
        let old: Seen = serde_json::from_str("{}").unwrap();
        assert_eq!(old, Seen::default());
    }
}
