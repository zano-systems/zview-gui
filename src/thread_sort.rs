//! View-local thread ordering, independent of incoming frame order.
use crate::model::{Frame, Thread};
use std::cmp::Ordering;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Column {
    Name,
    Cpu,
    Load,
    StackPercent,
    StackBytes,
}

impl Column {
    pub fn id(self) -> &'static str {
        match self {
            Self::Name => "thread-sort-name",
            Self::Cpu => "thread-sort-cpu",
            Self::Load => "thread-sort-load",
            Self::StackPercent => "thread-sort-stack-percent",
            Self::StackBytes => "thread-sort-stack-bytes",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Name => "THREAD",
            Self::Cpu => "CPU %",
            Self::Load => "LOAD %",
            Self::StackPercent => "STACK WATERMARK",
            Self::StackBytes => "WATERMARK / SIZE",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sort {
    pub column: Column,
    pub descending: bool,
}

impl Default for Sort {
    fn default() -> Self {
        Self {
            column: Column::Load,
            descending: true,
        }
    }
}

impl Sort {
    pub fn select(&mut self, column: Column) {
        self.descending = if self.column == column {
            !self.descending
        } else {
            column != Column::Name
        };
        self.column = column;
    }

    pub fn rows(self, frame: &Frame) -> Vec<&Thread> {
        let mut rows: Vec<_> = frame
            .threads
            .iter()
            .filter(|thread| !thread.is_idle())
            .collect();
        rows.sort_by(|a, b| {
            let order = match self.column {
                Column::Name => {
                    let order = a.name.cmp(&b.name);
                    if self.descending {
                        order.reverse()
                    } else {
                        order
                    }
                }
                Column::Cpu => optional(
                    a.runtime.as_ref().and_then(|r| r.cpu_normalized),
                    b.runtime.as_ref().and_then(|r| r.cpu_normalized),
                    self.descending,
                ),
                Column::Load => optional(
                    a.runtime.as_ref().and_then(|r| r.cpu),
                    b.runtime.as_ref().and_then(|r| r.cpu),
                    self.descending,
                ),
                Column::StackPercent => optional(
                    a.runtime.as_ref().and_then(|r| r.stack_watermark_percent),
                    b.runtime.as_ref().and_then(|r| r.stack_watermark_percent),
                    self.descending,
                ),
                Column::StackBytes => optional(
                    a.runtime.as_ref().and_then(|r| r.stack_watermark),
                    b.runtime.as_ref().and_then(|r| r.stack_watermark),
                    self.descending,
                ),
            };
            order
                .then_with(|| a.name.cmp(&b.name))
                .then_with(|| a.address.cmp(&b.address))
        });
        rows
    }
}

// Missing readings stay last in either direction, not equivalent to zero.
fn optional<T: PartialOrd>(a: Option<T>, b: Option<T>, descending: bool) -> Ordering {
    match (a, b) {
        (Some(a), Some(b)) => {
            let order = a.partial_cmp(&b).unwrap_or(Ordering::Equal);
            if descending { order.reverse() } else { order }
        }
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame() -> Frame {
        let mut frame = crate::model::parse(include_str!("../sample.json"))
            .unwrap()
            .remove(0);
        let template = frame.visible_threads()[0].clone();
        frame.threads = (0..4)
            .map(|index| {
                let mut thread = template.clone();
                thread.is_idle = Some(false);
                thread.address = index;
                thread.name = ["bravo", "alpha", "charlie", "alpha"][index as usize].into();
                thread.runtime.as_mut().unwrap().cpu =
                    [Some(10.), Some(30.), None, Some(30.)][index as usize];
                thread.runtime.as_mut().unwrap().cpu_normalized = Some(index as f64);
                thread
            })
            .collect();
        frame
    }

    fn addresses(sort: Sort, frame: &Frame) -> Vec<u64> {
        sort.rows(frame)
            .iter()
            .map(|thread| thread.address)
            .collect()
    }

    #[test]
    fn default_uses_load_with_stable_ties_and_missing_last() {
        let frame = frame();
        let mut sort = Sort::default();
        assert_eq!(addresses(sort, &frame), [1, 3, 0, 2]);
        sort.select(Column::Load);
        assert_eq!(addresses(sort, &frame), [0, 1, 3, 2]);
        sort.select(Column::Cpu);
        assert_eq!(addresses(sort, &frame), [3, 2, 1, 0]);
    }

    #[test]
    fn name_order_survives_new_readings_and_input_reordering() {
        let mut frame = frame();
        let mut sort = Sort::default();
        sort.select(Column::Name);
        assert_eq!(addresses(sort, &frame), [1, 3, 0, 2]);
        frame.threads.reverse();
        for thread in &mut frame.threads {
            thread.runtime.as_mut().unwrap().cpu = Some(thread.address as f64);
        }
        assert_eq!(addresses(sort, &frame), [1, 3, 0, 2]);
        sort.select(Column::Name);
        assert_eq!(addresses(sort, &frame), [2, 0, 1, 3]);
    }

    #[test]
    fn every_numeric_column_keeps_unknown_readings_last() {
        assert_eq!(optional(Some(0.), None, true), Ordering::Less);
        assert_eq!(optional(Some(0.), None, false), Ordering::Less);
        for column in [
            Column::Cpu,
            Column::Load,
            Column::StackPercent,
            Column::StackBytes,
        ] {
            let mut frame = frame();
            frame.threads[2].runtime = None;
            for descending in [false, true] {
                assert_eq!(
                    addresses(Sort { column, descending }, &frame).last(),
                    Some(&2)
                );
            }
        }
    }
}
