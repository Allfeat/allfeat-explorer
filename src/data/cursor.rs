//! Transparent cursor types for the cursor-based pagination surface.
//!
//! Every list endpoint under `/api/v1/` is cursor-paginated. Cursors are
//! *transparent* — the wire form of a cursor is a plain human-readable
//! string like `12345-3`, not base64/JSON — so a URL in a bug report
//! tells you everything about where pagination stopped. The grammar per
//! resource lives here, verified by unit tests; the full design is
//! documented in `docs/api-pagination-plan.md`.
//!
//! Handlers parse the `?cursor=` query param at the API boundary. A
//! parse failure surfaces as `400 Bad Request` — never silently treated
//! as "no cursor" — so a client whose cursor format drifts fails loudly
//! rather than looping on the first page.
//!
//! Each cursor is sorted newest-first; the query layer translates a
//! cursor into a strict `<` clause so the next page starts *just after*
//! the cursored row (the row the cursor identifies is the last one the
//! previous page returned).
//!
//! ```text
//! Resource                    Format                Example
//! ── ── ── ── ── ── ── ── ── ── ── ── ── ── ── ── ── ── ── ── ──
//! /blocks                     <block>               12345
//! /extrinsics                 <block>-<idx>         12345-3
//! /transfers                  <block>-<event_idx>   12345-7
//! /events                     <block>-<phase>-<idx> 12345-A-3
//! /ats, /accounts/.../ats     <id>                  42
//! /ats/feed                   <block>-<ats_id>-<ver> 100-42-3
//! ```

use std::fmt;
use std::str::FromStr;

use crate::domain::{EventPhase, PageRequest};

#[cfg(feature = "ssr")]
use crate::data::error::{DataError, DataResult};

/// Decode `req.cursor` as a typed cursor `C`. `None` when the caller
/// omitted the param; a parse failure surfaces as `BadRequest` so the
/// API layer maps to a 400 instead of silently treating a malformed
/// cursor as "no cursor".
#[cfg(feature = "ssr")]
pub fn parse_cursor<C>(req: &PageRequest) -> DataResult<Option<C>>
where
    C: FromStr,
    <C as FromStr>::Err: fmt::Display,
{
    let Some(raw) = req.cursor.as_deref() else {
        return Ok(None);
    };
    raw.parse::<C>()
        .map(Some)
        .map_err(|e| DataError::BadRequest(e.to_string()))
}

/// Failure to parse a cursor string. Carries both the offending input
/// and the expected grammar so handlers can render a precise 400.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CursorParseError {
    pub input: String,
    pub expected: &'static str,
}

impl fmt::Display for CursorParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "invalid cursor '{}': expected {}",
            self.input, self.expected
        )
    }
}

impl std::error::Error for CursorParseError {}

fn err(input: &str, expected: &'static str) -> CursorParseError {
    CursorParseError {
        input: input.to_string(),
        expected,
    }
}

// ── /blocks ─────────────────────────────────────────────────────────────────

/// Cursor for `/blocks`. Format: `<block_num>`. Next-page condition:
/// `block_num < cursor.block`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlockCursor {
    pub block: u64,
}

impl fmt::Display for BlockCursor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.block)
    }
}

impl FromStr for BlockCursor {
    type Err = CursorParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.parse::<u64>()
            .map(|block| BlockCursor { block })
            .map_err(|_| err(s, "<block_num> (u64)"))
    }
}

// ── /extrinsics ─────────────────────────────────────────────────────────────

/// Cursor for `/extrinsics`. Format: `<block>-<idx>`. Next-page
/// condition: `(block, idx) < (cursor.block, cursor.index)` lexico.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExtrinsicCursor {
    pub block: u64,
    pub index: u32,
}

impl fmt::Display for ExtrinsicCursor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-{}", self.block, self.index)
    }
}

impl FromStr for ExtrinsicCursor {
    type Err = CursorParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (b, i) = s.split_once('-').ok_or_else(|| err(s, "<block>-<idx>"))?;
        let block = b
            .parse::<u64>()
            .map_err(|_| err(s, "<block>-<idx> (block must be u64)"))?;
        let index = i
            .parse::<u32>()
            .map_err(|_| err(s, "<block>-<idx> (idx must be u32)"))?;
        Ok(ExtrinsicCursor { block, index })
    }
}

// ── /transfers ──────────────────────────────────────────────────────────────

/// Cursor for `/transfers`. Format: `<block>-<event_idx>`. Transfers
/// are keyed by the `balances.Transfer` event rather than by the
/// enclosing extrinsic (batched transfers produce multiple events in a
/// single extrinsic), so ordering uses the event index within the block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TransferCursor {
    pub block: u64,
    pub event_idx: u32,
}

impl fmt::Display for TransferCursor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-{}", self.block, self.event_idx)
    }
}

impl FromStr for TransferCursor {
    type Err = CursorParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (b, e) = s
            .split_once('-')
            .ok_or_else(|| err(s, "<block>-<event_idx>"))?;
        let block = b
            .parse::<u64>()
            .map_err(|_| err(s, "<block>-<event_idx> (block must be u64)"))?;
        let event_idx = e
            .parse::<u32>()
            .map_err(|_| err(s, "<block>-<event_idx> (event_idx must be u32)"))?;
        Ok(TransferCursor { block, event_idx })
    }
}

// ── /events ─────────────────────────────────────────────────────────────────

/// Block-phase letter for [`EventCursor`]. Mirrors `EventPhase` minus
/// the extrinsic-index payload, which the cursor doesn't need — event
/// ordering inside a block is already total via the event `index`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventPhaseKind {
    Initialization,
    ApplyExtrinsic,
    Finalization,
}

impl EventPhaseKind {
    pub fn letter(self) -> char {
        match self {
            EventPhaseKind::Initialization => 'I',
            EventPhaseKind::ApplyExtrinsic => 'A',
            EventPhaseKind::Finalization => 'F',
        }
    }

    pub fn from_letter(c: char) -> Option<Self> {
        Some(match c {
            'I' => EventPhaseKind::Initialization,
            'A' => EventPhaseKind::ApplyExtrinsic,
            'F' => EventPhaseKind::Finalization,
            _ => return None,
        })
    }
}

impl From<EventPhase> for EventPhaseKind {
    fn from(phase: EventPhase) -> Self {
        match phase {
            EventPhase::Initialization => EventPhaseKind::Initialization,
            EventPhase::ApplyExtrinsic { .. } => EventPhaseKind::ApplyExtrinsic,
            EventPhase::Finalization => EventPhaseKind::Finalization,
        }
    }
}

/// Cursor for `/events`. Format: `<block>-<phase>-<idx>` where `<phase>`
/// is one of `I`/`A`/`F`. Ordering is `(block, idx) < (cursor.block,
/// cursor.index)`; the phase letter is informational (for URL
/// readability) and not part of the ordering key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EventCursor {
    pub block: u64,
    pub phase: EventPhaseKind,
    pub index: u32,
}

impl fmt::Display for EventCursor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-{}-{}", self.block, self.phase.letter(), self.index)
    }
}

impl FromStr for EventCursor {
    type Err = CursorParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        const EXPECTED: &str = "<block>-<phase>-<idx> with phase in I|A|F";
        let mut parts = s.splitn(3, '-');
        let b = parts.next().ok_or_else(|| err(s, EXPECTED))?;
        let p = parts.next().ok_or_else(|| err(s, EXPECTED))?;
        let i = parts.next().ok_or_else(|| err(s, EXPECTED))?;
        if parts.next().is_some() {
            return Err(err(s, EXPECTED));
        }
        let block = b.parse::<u64>().map_err(|_| err(s, EXPECTED))?;
        let phase = match p.chars().next() {
            Some(c) if p.len() == 1 => {
                EventPhaseKind::from_letter(c).ok_or_else(|| err(s, EXPECTED))?
            }
            _ => return Err(err(s, EXPECTED)),
        };
        let index = i.parse::<u32>().map_err(|_| err(s, EXPECTED))?;
        Ok(EventCursor {
            block,
            phase,
            index,
        })
    }
}

// ── /ats and /accounts/{addr}/ats ───────────────────────────────────────────

/// Cursor for `/ats` (and `/accounts/{address}/ats`). Format: `<id>`.
/// Next-page condition: `ats_id < cursor.id`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AtsCursor {
    pub id: u32,
}

impl fmt::Display for AtsCursor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.id)
    }
}

impl FromStr for AtsCursor {
    type Err = CursorParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.parse::<u32>()
            .map(|id| AtsCursor { id })
            .map_err(|_| err(s, "<id> (u32)"))
    }
}

// ── /ats/feed ───────────────────────────────────────────────────────────────

/// Cursor for `/ats/feed`. Format: `<block_num>-<ats_id>-<version>`.
/// Ordering is strict time-first newest-first:
/// `(block_num, ats_id, version) < cursor` lexico. The `block_num`
/// component is what lets a new version of an old ATS sort to the top
/// alongside fresh registrations from the same block window — a sort by
/// `ats_id` alone would push the update behind every later-registered
/// ATS even though it's chronologically newer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AtsFeedCursor {
    pub block_num: u64,
    pub ats_id: u32,
    pub version: u32,
}

impl fmt::Display for AtsFeedCursor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-{}-{}", self.block_num, self.ats_id, self.version)
    }
}

impl FromStr for AtsFeedCursor {
    type Err = CursorParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        const EXPECTED: &str = "<block_num>-<ats_id>-<version>";
        let mut parts = s.splitn(3, '-');
        let b = parts.next().ok_or_else(|| err(s, EXPECTED))?;
        let a = parts.next().ok_or_else(|| err(s, EXPECTED))?;
        let v = parts.next().ok_or_else(|| err(s, EXPECTED))?;
        if parts.next().is_some() {
            return Err(err(s, EXPECTED));
        }
        let block_num = b
            .parse::<u64>()
            .map_err(|_| err(s, "<block_num>-<ats_id>-<version> (block_num must be u64)"))?;
        let ats_id = a
            .parse::<u32>()
            .map_err(|_| err(s, "<block_num>-<ats_id>-<version> (ats_id must be u32)"))?;
        let version = v
            .parse::<u32>()
            .map_err(|_| err(s, "<block_num>-<ats_id>-<version> (version must be u32)"))?;
        Ok(AtsFeedCursor {
            block_num,
            ats_id,
            version,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip<C>(c: C, expected: &str)
    where
        C: fmt::Display + FromStr + PartialEq + fmt::Debug + Copy,
        <C as FromStr>::Err: fmt::Debug,
    {
        assert_eq!(c.to_string(), expected);
        let parsed: C = expected.parse().expect("roundtrip parse");
        assert_eq!(parsed, c);
    }

    #[test]
    fn block_cursor_roundtrips() {
        roundtrip(BlockCursor { block: 0 }, "0");
        roundtrip(BlockCursor { block: 12345 }, "12345");
        roundtrip(BlockCursor { block: u64::MAX }, &u64::MAX.to_string());
    }

    #[test]
    fn block_cursor_rejects_garbage() {
        assert!("".parse::<BlockCursor>().is_err());
        assert!("abc".parse::<BlockCursor>().is_err());
        assert!("-1".parse::<BlockCursor>().is_err());
        assert!("12-3".parse::<BlockCursor>().is_err());
        assert!(" 12 ".parse::<BlockCursor>().is_err());
    }

    #[test]
    fn extrinsic_cursor_roundtrips() {
        roundtrip(
            ExtrinsicCursor {
                block: 12345,
                index: 0,
            },
            "12345-0",
        );
        roundtrip(
            ExtrinsicCursor {
                block: 12345,
                index: 3,
            },
            "12345-3",
        );
    }

    #[test]
    fn extrinsic_cursor_rejects_garbage() {
        assert!("".parse::<ExtrinsicCursor>().is_err());
        assert!("12345".parse::<ExtrinsicCursor>().is_err());
        assert!("12345-".parse::<ExtrinsicCursor>().is_err());
        assert!("-3".parse::<ExtrinsicCursor>().is_err());
        assert!("abc-3".parse::<ExtrinsicCursor>().is_err());
        assert!("12345-xyz".parse::<ExtrinsicCursor>().is_err());
    }

    #[test]
    fn transfer_cursor_roundtrips() {
        roundtrip(
            TransferCursor {
                block: 12345,
                event_idx: 7,
            },
            "12345-7",
        );
    }

    #[test]
    fn transfer_cursor_rejects_garbage() {
        assert!("12345".parse::<TransferCursor>().is_err());
        assert!("12345--7".parse::<TransferCursor>().is_err());
    }

    #[test]
    fn event_cursor_roundtrips() {
        roundtrip(
            EventCursor {
                block: 12345,
                phase: EventPhaseKind::ApplyExtrinsic,
                index: 3,
            },
            "12345-A-3",
        );
        roundtrip(
            EventCursor {
                block: 12345,
                phase: EventPhaseKind::Initialization,
                index: 0,
            },
            "12345-I-0",
        );
        roundtrip(
            EventCursor {
                block: 12345,
                phase: EventPhaseKind::Finalization,
                index: 9,
            },
            "12345-F-9",
        );
    }

    #[test]
    fn event_cursor_rejects_garbage() {
        assert!("".parse::<EventCursor>().is_err());
        assert!("12345".parse::<EventCursor>().is_err());
        assert!("12345-X-3".parse::<EventCursor>().is_err());
        assert!("12345-AA-3".parse::<EventCursor>().is_err());
        assert!("12345-A-3-extra".parse::<EventCursor>().is_err());
        assert!("abc-A-3".parse::<EventCursor>().is_err());
        assert!("12345-A-xyz".parse::<EventCursor>().is_err());
    }

    #[test]
    fn event_phase_kind_from_domain_phase() {
        assert_eq!(
            EventPhaseKind::from(EventPhase::Initialization),
            EventPhaseKind::Initialization
        );
        assert_eq!(
            EventPhaseKind::from(EventPhase::ApplyExtrinsic { index: 7 }),
            EventPhaseKind::ApplyExtrinsic,
        );
        assert_eq!(
            EventPhaseKind::from(EventPhase::Finalization),
            EventPhaseKind::Finalization
        );
    }

    #[test]
    fn ats_cursor_roundtrips() {
        roundtrip(AtsCursor { id: 0 }, "0");
        roundtrip(AtsCursor { id: 42 }, "42");
        roundtrip(AtsCursor { id: u32::MAX }, &u32::MAX.to_string());
    }

    #[test]
    fn ats_cursor_rejects_garbage() {
        assert!("".parse::<AtsCursor>().is_err());
        assert!("abc".parse::<AtsCursor>().is_err());
        assert!("-1".parse::<AtsCursor>().is_err());
    }

    #[test]
    fn ats_feed_cursor_roundtrips() {
        roundtrip(
            AtsFeedCursor {
                block_num: 100,
                ats_id: 42,
                version: 3,
            },
            "100-42-3",
        );
        roundtrip(
            AtsFeedCursor {
                block_num: 0,
                ats_id: 0,
                version: 0,
            },
            "0-0-0",
        );
    }

    #[test]
    fn ats_feed_cursor_rejects_garbage() {
        assert!("42".parse::<AtsFeedCursor>().is_err());
        assert!("42-3".parse::<AtsFeedCursor>().is_err());
        assert!("42-".parse::<AtsFeedCursor>().is_err());
        assert!("-3".parse::<AtsFeedCursor>().is_err());
        assert!("abc-3-1".parse::<AtsFeedCursor>().is_err());
        assert!("1-2-3-4".parse::<AtsFeedCursor>().is_err());
    }

    #[test]
    fn error_display_includes_input_and_expected() {
        let e = "bogus".parse::<ExtrinsicCursor>().unwrap_err();
        let msg = e.to_string();
        assert!(
            msg.contains("bogus"),
            "error should name the bad input: {msg}"
        );
        assert!(
            msg.contains("<block>-<idx>"),
            "error should name the grammar: {msg}"
        );
    }
}
