//! Worker-thread console access to the same public observation API as modules.
use super::*;
use std::sync::{LazyLock, Mutex};

static WATCHES: LazyLock<Mutex<Vec<(Topic, Observation)>>> =
    LazyLock::new(|| Mutex::new(Vec::new()));
const TOPICS: [(Topic, &str); 6] = [
    (Topic::Entities, "entities"),
    (Topic::Inventory, "inventory"),
    (Topic::Vitals, "vitals"),
    (Topic::Status, "status"),
    (Topic::World, "world"),
    (Topic::Target, "target"),
];
pub(super) fn clear() {
    WATCHES.lock().unwrap_or_else(|e| e.into_inner()).clear();
}
pub(crate) fn command(action: &str, name: Option<&str>) -> String {
    if action == "unwatch" {
        clear();
        return "SDK: 読み取りの購読を解除しました".into();
    }
    let game = match game() {
        Ok(game) => game,
        Err(error) => return format!("SDK: {error}"),
    };
    let mut watches = WATCHES.lock().unwrap_or_else(|e| e.into_inner());
    if action == "watch" {
        let name = name.unwrap_or("all");
        if name != "all" && !TOPICS.iter().any(|(_, n)| *n == name) {
            return "sdk watch all|entities|inventory|vitals|status|world|target".into();
        }
        let mut lines = Vec::new();
        for (topic, label) in TOPICS {
            if name != "all" && name != label {
                continue;
            }
            if watches.iter().any(|(old, _)| *old == topic) {
                continue;
            }
            match game.observe(topic) {
                Ok(reader) => {
                    watches.push((topic, reader));
                    lines.push(format!("SDK {label}: 購読を開始しました"));
                }
                Err(error) => lines.push(format!("SDK {label}: {error}")),
            }
        }
        lines.push("メニューを閉じて取得を待ち、sdk inspectで確認してください".into());
        return lines.join("\n");
    }
    if watches.is_empty() {
        return "SDK: sdk watch allで購読を開始してください".into();
    }
    watches
        .iter()
        .map(|(topic, reader)| match reader.read() {
            Err(error) => format!("SDK {topic:?}: {error}"),
            Ok(reading) => {
                let text = match reading.data {
                    ObservationData::Entities(v) => format!(
                        "{} entities; {:?}",
                        v.len(),
                        v.iter()
                            .take(8)
                            .map(|e| (e.id, e.name()))
                            .collect::<Vec<_>>()
                    ),
                    ObservationData::Inventory(v) => format!(
                        "selected={} bread={} slots={} armor={:?}; hotbar={:?}",
                        v.selected,
                        v.count("bread"),
                        v.slots.len(),
                        v.armor,
                        &v.slots[..v.slots.len().min(9)]
                    ),
                    ObservationData::Vitals(v) => format!("{v:?}"),
                    ObservationData::Status(v) => format!("{v:?}"),
                    ObservationData::World(v) => {
                        format!("{v:?}; day={} clock={:?}", v.day(), v.clock())
                    }
                    ObservationData::Target(v) => format!("{v:?}"),
                };
                format!("SDK {topic:?}: age={}ms {text}", reading.age_ms)
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}
