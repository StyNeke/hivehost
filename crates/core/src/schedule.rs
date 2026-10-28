// Weekly recurrences at a wall-clock time in an IANA time zone, for scheduled restarts, backups,
// commands and updates. Daylight-saving changes follow the zone: a time skipped by a spring
// change runs at the first valid moment after it, and a time repeated in autumn runs once.
use jiff::{Timestamp, ToSpan, civil, tz::TimeZone};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Recurrence {
    pub hour: u8,
    pub minute: u8,
    // Bit 0 is Monday, bit 6 is Sunday.
    pub weekdays: u8,
    pub timezone: String,
}

impl Recurrence {
    pub fn validate(&self) -> Result<(), String> {
        if self.hour > 23 || self.minute > 59 {
            return Err("choose a time between 00:00 and 23:59".into());
        }
        if self.weekdays == 0 || self.weekdays > 0x7f {
            return Err("choose at least one day of the week".into());
        }
        if self.timezone.is_empty()
            || self.timezone.len() > 64
            || !self
                .timezone
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"/_+-".contains(&b))
        {
            return Err("unknown time zone".into());
        }
        TimeZone::get(&self.timezone).map_err(|_| "unknown time zone".to_string())?;
        Ok(())
    }

    // The first run strictly after `after` (Unix seconds).
    pub fn next_after(&self, after: i64) -> Result<i64, String> {
        self.validate()?;
        let zone = TimeZone::get(&self.timezone).map_err(|_| "unknown time zone".to_string())?;
        let start = Timestamp::from_second(after)
            .map_err(|_| "time out of range".to_string())?
            .to_zoned(zone.clone());
        for offset in 0..=7i32 {
            let date = start
                .date()
                .checked_add(offset.days())
                .map_err(|_| "time out of range".to_string())?;
            let bit = date.weekday().to_monday_zero_offset() as u8;
            if self.weekdays & (1 << bit) == 0 {
                continue;
            }
            let wall = civil::DateTime::new(
                date.year(),
                date.month(),
                date.day(),
                self.hour as i8,
                self.minute as i8,
                0,
                0,
            )
            .map_err(|_| "invalid time".to_string())?;
            let at = zone
                .to_ambiguous_zoned(wall)
                .compatible()
                .map_err(|_| "invalid time".to_string())?
                .timestamp()
                .as_second();
            if at > after {
                return Ok(at);
            }
        }
        Err("no upcoming run".into())
    }
}
