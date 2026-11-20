//! The cron-like schedule of the job editor, and its conversion to and from
//! launchd's `StartCalendarInterval`.

use std::collections::{BTreeMap, BTreeSet};

use plist::Value;
use serde::{Deserialize, Serialize};

use super::error::LaunchdError;

/// The maximum number of start times a calendar may expand to: launchd takes
/// each combination of the fields' values as an entry.
pub const MAX_CALENDAR_ENTRIES: usize = 1000;

/// A schedule in the fields of cron, of which each is a number, a list
/// (`1,15`), a range (`1-5`), an interval (`*/10`), or `*` or `""` for any.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Calendar {
    pub minute: String,
    pub hour: String,
    pub day: String,
    pub month: String,
    pub weekday: String,
}

/// One expanded `StartCalendarInterval` entry.
pub type CalendarEntry = BTreeMap<String, i32>;

struct CalendarField {
    key: &'static str,
    min: i32,
    max: i32,
}

const CALENDAR_FIELDS: [CalendarField; 5] = [
    CalendarField {
        key: "Minute",
        min: 0,
        max: 59,
    },
    CalendarField {
        key: "Hour",
        min: 0,
        max: 23,
    },
    CalendarField {
        key: "Day",
        min: 1,
        max: 31,
    },
    CalendarField {
        key: "Month",
        min: 1,
        max: 12,
    },
    CalendarField {
        key: "Weekday",
        min: 0,
        max: 7,
    },
];

impl Calendar {
    fn values(&self) -> [&str; 5] {
        [
            &self.minute,
            &self.hour,
            &self.day,
            &self.month,
            &self.weekday,
        ]
    }

    /// Whether no field is set: there is no calendar.
    pub fn empty(&self) -> bool {
        self.values().iter().all(|value| value.trim().is_empty())
    }

    /// The calendar as launchd's `StartCalendarInterval`: entries leave out
    /// the fields of any value. An empty calendar has no entries.
    pub fn entries(&self) -> Result<Vec<CalendarEntry>, LaunchdError> {
        if self.empty() {
            return Ok(Vec::new());
        }
        let mut entries: Vec<CalendarEntry> = vec![BTreeMap::new()];
        for (index, text) in self.values().into_iter().enumerate() {
            let field = &CALENDAR_FIELDS[index];
            let values = parse_field(field, text)?;
            let Some(values) = values else {
                continue;
            };
            if entries.len() * values.len() > MAX_CALENDAR_ENTRIES {
                return Err(LaunchdError::validation(format!(
                    "the calendar has more than {MAX_CALENDAR_ENTRIES} start times: use fewer values, or an interval"
                )));
            }
            let mut next = Vec::with_capacity(entries.len() * values.len());
            for entry in &entries {
                for value in &values {
                    let mut expanded = entry.clone();
                    expanded.insert(field.key.to_string(), *value);
                    next.push(expanded);
                }
            }
            entries = next;
        }
        // Any value in every field is every minute, which launchd's
        // documentation only describes through missing keys of an entry that
        // has some.
        if entries[0].is_empty() {
            entries = (0..60)
                .map(|minute| {
                    let mut entry = BTreeMap::new();
                    entry.insert("Minute".to_string(), minute);
                    entry
                })
                .collect();
        }
        Ok(entries)
    }
}

/// The values a field's text stands for, or `None` for any value.
fn parse_field(field: &CalendarField, text: &str) -> Result<Option<Vec<i32>>, LaunchdError> {
    let name = field.key.to_lowercase();
    let text = text.trim();
    if text.is_empty() || text == "*" {
        return Ok(None);
    }
    let number = |part: &str| -> Result<i32, LaunchdError> {
        let trimmed = part.trim();
        let value = trimmed.parse::<i32>().map_err(|_| {
            LaunchdError::validation(format!(
                "the {name} {text:?} is not a number, a list, a range or an interval"
            ))
        })?;
        if value < field.min || value > field.max {
            return Err(LaunchdError::validation(format!(
                "the {name} {value} is outside {}-{}",
                field.min, field.max
            )));
        }
        Ok(value)
    };
    let mut values = Vec::new();
    for part in text.split(',') {
        let part = part.trim();
        if let Some(step) = part.strip_prefix("*/") {
            let step = step.parse::<i32>().unwrap_or(0);
            if step < 1 {
                return Err(LaunchdError::validation(format!(
                    "the {name} interval {part:?} is not a positive number"
                )));
            }
            if step == 1 && part == text {
                return Ok(None);
            }
            let mut value = field.min;
            while value <= field.max {
                values.push(value);
                value += step;
            }
            continue;
        }
        let (first, last, is_range) = match part.split_once('-') {
            Some((first, last)) => (first, last, true),
            None => (part, "", false),
        };
        let start = number(first)?;
        let end = if is_range {
            let end = number(last)?;
            if end < start {
                return Err(LaunchdError::validation(format!(
                    "the {name} range {part:?} goes backwards"
                )));
            }
            end
        } else {
            start
        };
        for value in start..=end {
            values.push(value);
        }
    }
    values.sort_unstable();
    values.dedup();
    Ok(Some(values))
}

/// Writes values as the text of a field, the shortest way.
fn format_field(field: &CalendarField, values: &[i32]) -> String {
    if values.len() >= 3 && values[0] == field.min {
        let step = values[1] - values[0];
        let mut regular = step > 1 && values[values.len() - 1] + step > field.max;
        for pair in values.windows(2) {
            regular = regular && pair[1] - pair[0] == step;
        }
        if regular {
            return format!("*/{step}");
        }
    }
    let mut parts = Vec::new();
    let mut index = 0;
    while index < values.len() {
        let mut end = index;
        while end + 1 < values.len() && values[end + 1] == values[end] + 1 {
            end += 1;
        }
        if end - index >= 2 {
            parts.push(format!("{}-{}", values[index], values[end]));
        } else {
            for value in &values[index..=end] {
                parts.push(value.to_string());
            }
        }
        index = end + 1;
    }
    parts.join(",")
}

/// Reads `StartCalendarInterval` if the fields of a [`Calendar`] can say what
/// it says: its entries must be all the combinations of their values.
pub fn calendar_from(value: &Value) -> Option<Calendar> {
    let raw: Vec<&Value> = match value {
        Value::Dictionary(_) => vec![value],
        Value::Array(values) => values.iter().collect(),
        _ => return None,
    };
    if raw.is_empty() {
        return None;
    }
    let mut seen: BTreeSet<[i32; 5]> = BTreeSet::new();
    let mut sets: [Vec<i32>; 5] = Default::default();
    let mut present = [false; 5];
    for (index, item) in raw.iter().enumerate() {
        let entry = match item {
            Value::Dictionary(entry) if !entry.is_empty() => entry,
            _ => return None,
        };
        let mut known = 0;
        let mut key = [-1_i32; 5];
        for (field_index, field) in CALENDAR_FIELDS.iter().enumerate() {
            let has = entry.contains_key(field.key);
            if index == 0 {
                present[field_index] = has;
            }
            if has != present[field_index] {
                return None;
            }
            if !has {
                continue;
            }
            known += 1;
            let number = plist_int(entry.get(field.key)?)?;
            if number < field.min || number > field.max {
                return None;
            }
            key[field_index] = number;
            if !sets[field_index].contains(&number) {
                sets[field_index].push(number);
            }
        }
        if known != entry.len() || !seen.insert(key) {
            return None;
        }
    }
    let mut product = 1usize;
    for index in 0..5 {
        if present[index] {
            product *= sets[index].len();
        }
    }
    if product != raw.len() {
        return None;
    }
    let mut text = [
        String::new(),
        String::new(),
        String::new(),
        String::new(),
        String::new(),
    ];
    for (index, field) in CALENDAR_FIELDS.iter().enumerate() {
        if present[index] {
            sets[index].sort_unstable();
            text[index] = format_field(field, &sets[index]);
        }
    }
    let [minute, hour, day, month, weekday] = text;
    Some(Calendar {
        minute,
        hour,
        day,
        month,
        weekday,
    })
}

fn plist_int(value: &Value) -> Option<i32> {
    match value {
        Value::Integer(number) => number
            .as_signed()
            .and_then(|n| i32::try_from(n).ok())
            .or_else(|| number.as_unsigned().and_then(|n| i32::try_from(n).ok())),
        _ => None,
    }
}
