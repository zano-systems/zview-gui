//! Value rules shared by the native and browser session forms.
use crate::adapter_protocol::{BuildContext, LiveConfiguration, Mode};
use serde_json::{Value, json};

/// Only associate connected-firmware discovery with the same effective settings.
pub(crate) fn matches_discovery(
    request: &Value,
    context: &BuildContext,
    configuration: &LiveConfiguration,
) -> bool {
    *request
        == AdvancedValues::accepted(context, configuration).request(
            Mode::Live,
            &configuration.build_dir,
            None,
        )
}

/// Capabilities list available groups, not the groups selected for polling.
/// Keep known absent groups visible and preserve future backend group names.
pub(crate) fn discovery_groups(
    capabilities: &crate::adapter_protocol::Capabilities,
) -> Vec<(String, bool)> {
    let mut names = vec!["threads".to_owned()];
    names.extend(crate::kernel_model::Kind::ALL.map(|kind| kind.group().to_owned()));
    for group in &capabilities.groups {
        if !names.contains(group) {
            names.push(group.clone());
        }
    }
    names
        .into_iter()
        .map(|name| {
            let available = capabilities.groups.contains(&name);
            (name, available)
        })
        .collect()
}

pub(crate) fn discovery_name(group: &str) -> String {
    match group {
        "threads" => "Threads",
        "heaps" => "Heaps",
        "mem_slabs" => "Memory slabs",
        "msgqs" => "Message queues",
        "mutexes" => "Mutexes",
        "semaphores" => "Semaphores",
        "workqs" => "Work queues",
        _ => group,
    }
    .to_owned()
}

/// Keep tooltips bounded; absence of inventory is not an empty group.
pub(crate) fn discovery_tooltip(
    name: &str,
    available: bool,
    objects: Option<&[crate::adapter_protocol::HeapIdentity]>,
) -> String {
    if !available {
        return format!("{name}: unavailable in the connected firmware.");
    }
    let Some(objects) = objects else {
        return format!(
            "{name}: available in the connected firmware.\nObject inventory has not been received."
        );
    };
    let mut text = format!("{name}: {} discovered objects", objects.len());
    for object in objects.iter().take(10) {
        text.push_str(&format!(
            "\n• {} · 0x{:x}",
            if object.name.is_empty() {
                "Unnamed"
            } else {
                &object.name
            },
            object.address
        ));
    }
    if objects.len() > 10 {
        text.push_str(&format!(
            "\n… and {} more. Open the corresponding screen to inspect.",
            objects.len() - 10
        ));
    }
    text
}

/// Field order matches the advanced inputs: image, ELF, runner, target, clock.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct AdvancedValues(pub [String; 5]);

impl AdvancedValues {
    pub fn detected(context: &BuildContext) -> Self {
        Self([
            context.domain.clone(),
            context.elf.clone(),
            context.runner.clone().unwrap_or_default(),
            context.target.clone().unwrap_or_default(),
            context.frequency.map(|v| v.to_string()).unwrap_or_default(),
        ])
    }

    pub fn accepted(context: &BuildContext, configuration: &LiveConfiguration) -> Self {
        let mut values = Self::detected(context);
        for (index, value) in [
            configuration.domain.as_ref(),
            configuration.elf.as_ref(),
            configuration.runner.as_ref(),
            configuration.target.as_ref(),
        ]
        .into_iter()
        .enumerate()
        {
            if let Some(value) = value {
                values.0[index] = value.clone();
            }
        }
        if let Some(frequency) = configuration.frequency {
            values.0[4] = frequency.to_string();
        }
        values
    }

    /// Background detection replaces only fields unchanged since their last fill.
    /// Refresh deliberately replaces every field. Call only after successful resolution.
    pub fn merge(&self, baseline: &mut Self, detected: &Self, refresh: bool) -> Self {
        Self(std::array::from_fn(|i| {
            if refresh || self.0[i] == baseline.0[i] {
                baseline.0[i] = detected.0[i].clone();
                detected.0[i].clone()
            } else {
                self.0[i].clone()
            }
        }))
    }

    /// A baseline excludes detected values from discovery requests so changing an
    /// image or runner can also re-detect its dependent settings.
    pub fn request(&self, mode: Mode, build: &str, baseline: Option<&Self>) -> Value {
        let mut request = json!({"mode": mode, "build_dir": build});
        for (i, key) in ["domain", "elf", "runner", "target", "frequency"]
            .into_iter()
            .enumerate()
        {
            let value = &self.0[i];
            if value.is_empty()
                || (mode != Mode::Live && i >= 2)
                || baseline.is_some_and(|baseline| baseline.0[i] == *value)
            {
                continue;
            }
            request[key] = if i == 4 {
                value.parse::<u64>().map_or(json!(value), |v| json!(v))
            } else {
                json!(value)
            };
        }
        request
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values() -> AdvancedValues {
        AdvancedValues(["app", "build/app.elf", "jlink", "mcu", "4000000"].map(str::to_owned))
    }

    #[test]
    fn discovery_distinguishes_absent_groups_and_preserves_new_groups() {
        let capabilities = crate::adapter_protocol::Capabilities {
            groups: vec!["threads".into(), "heaps".into(), "future".into()],
            missing: vec![],
            is_waitq_walkable: false,
        };
        let groups = discovery_groups(&capabilities);
        assert_eq!(groups.len(), 8);
        assert!(groups.contains(&("threads".into(), true)));
        assert!(groups.contains(&("heaps".into(), true)));
        assert!(groups.contains(&("mutexes".into(), false)));
        assert!(groups.contains(&("future".into(), true)));
    }

    #[test]
    fn discovery_tooltips_distinguish_unknown_empty_and_bounded_inventory() {
        use crate::adapter_protocol::HeapIdentity;
        assert!(discovery_tooltip("Heaps", true, None).contains("not been received"));
        assert!(discovery_tooltip("Heaps", true, Some(&[])).contains("0 discovered objects"));
        assert!(discovery_tooltip("Heaps", false, None).contains("unavailable"));
        let objects: Vec<_> = (0..12)
            .map(|i| HeapIdentity {
                address: i,
                name: format!("heap-{i}"),
            })
            .collect();
        let text = discovery_tooltip("Heaps", true, Some(&objects));
        assert!(text.contains("12 discovered objects"));
        assert!(text.contains("• heap-0 · 0x0"));
        assert!(text.contains("• heap-9 · 0x9"));
        assert!(!text.contains("• heap-10"));
        assert!(text.contains("and 2 more"));
        assert_eq!(discovery_name("mem_slabs"), "Memory slabs");
    }

    #[test]
    fn background_fill_preserves_edits_including_cleared_fields() {
        let mut baseline = values();
        let mut edited = baseline.clone();
        edited.0[2] = "pyocd".into();
        edited.0[3].clear();
        let mut detected = baseline.clone();
        detected.0[1] = "build/new.elf".into();
        detected.0[3] = "new-mcu".into();
        let merged = edited.merge(&mut baseline, &detected, false);
        assert_eq!(merged.0[1], "build/new.elf");
        assert_eq!(merged.0[2], "pyocd");
        assert_eq!(merged.0[3], "");
        let again = merged.merge(&mut baseline, &detected, false);
        assert_eq!(again, merged);
        assert_eq!(edited.merge(&mut baseline, &detected, true), detected);
    }

    #[test]
    fn initial_fill_and_new_build_use_empty_baseline() {
        let empty = AdvancedValues::default();
        assert_eq!(empty.merge(&mut empty.clone(), &values(), false), values());
        assert_eq!(
            empty.request(Mode::Live, "other-build", Some(&empty)),
            json!({"mode": "live", "build_dir": "other-build"})
        );
    }

    #[test]
    fn discovery_omits_detected_values_but_submission_includes_them() {
        let baseline = values();
        let mut edited = baseline.clone();
        edited.0[2] = "pyocd".into();
        assert_eq!(
            edited.request(Mode::Live, "build", Some(&baseline)),
            json!({"mode": "live", "build_dir": "build", "runner": "pyocd"})
        );
        let submitted = edited.request(Mode::Live, "build", None);
        assert_eq!(submitted["elf"], "build/app.elf");
        assert_eq!(submitted["frequency"], 4000000);
        edited.0[4] = "invalid".into();
        assert_eq!(
            edited.request(Mode::Live, "build", None)["frequency"],
            "invalid"
        );
    }

    #[test]
    fn replay_keeps_image_and_elf_without_probe_settings() {
        assert_eq!(
            values().request(Mode::Replay, "", None),
            json!({"mode": "replay", "build_dir": "", "domain": "app", "elf": "build/app.elf"})
        );
    }

    #[test]
    fn accepted_session_restores_effective_values_and_explicit_overrides() {
        let context: BuildContext = serde_json::from_value(json!({
            "build_dir": "build", "image_dir": "build", "domain": "app",
            "domains": ["app"], "elf": "build/app.elf", "board": null,
            "project": "example", "source_dir": null, "git": null,
            "runner": "jlink", "target": "mcu", "frequency": 4000000,
            "warnings": []
        }))
        .unwrap();
        let mut configuration = LiveConfiguration {
            build_dir: "build".into(),
            domain: None,
            elf: None,
            runner: None,
            target: None,
            frequency: None,
        };
        assert_eq!(AdvancedValues::accepted(&context, &configuration), values());
        let request = values().request(Mode::Live, "build", None);
        assert!(matches_discovery(&request, &context, &configuration));
        for (key, value) in [
            ("mode", json!("replay")),
            ("build_dir", json!("other-build")),
            ("domain", json!("other-image")),
            ("elf", json!("other.elf")),
            ("runner", json!("pyocd")),
            ("target", json!("other-mcu")),
            ("frequency", json!(1000000)),
        ] {
            let mut changed = request.clone();
            changed[key] = value;
            assert!(
                !matches_discovery(&changed, &context, &configuration),
                "{key}"
            );
        }
        configuration.runner = Some("pyocd".into());
        configuration.frequency = Some(1000000);
        let restored = AdvancedValues::accepted(&context, &configuration);
        assert_eq!(restored.0[2], "pyocd");
        assert_eq!(restored.0[4], "1000000");
        // A restored session is preserved like manual input until explicit refresh.
        assert_eq!(
            restored.merge(&mut AdvancedValues::default(), &values(), false),
            restored
        );
    }
}
