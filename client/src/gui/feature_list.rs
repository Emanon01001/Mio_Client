//! Presentation order only: indices always refer to the original module list.
use std::cmp::Ordering;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum Sort {
    #[default]
    Default,
    Name,
    NameDescending,
    Enabled,
    Category,
}
impl Sort {
    pub const ALL: [Self; 5] = [
        Self::Default,
        Self::Name,
        Self::NameDescending,
        Self::Enabled,
        Self::Category,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::Default => "標準の並び順",
            Self::Name => "名前（英字A→Z）",
            Self::NameDescending => "名前（英字Z→A）",
            Self::Enabled => "有効な機能を先頭に",
            Self::Category => "種類別",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Category {
    Movement,
    Combat,
    World,
    Visual,
    Hud,
    Other,
}
impl Category {
    pub const ALL: [Self; 6] = [
        Self::Movement,
        Self::Combat,
        Self::World,
        Self::Visual,
        Self::Hud,
        Self::Other,
    ];
    pub fn short_label(self) -> &'static str {
        match self {
            Self::Movement => "移動",
            Self::Combat => "戦闘",
            Self::World => "ワールド",
            Self::Visual => "視界",
            Self::Hud => "HUD",
            Self::Other => "その他",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Movement => "移動・操作補助",
            Self::Combat => "戦闘",
            Self::World => "ワールド・所持品",
            Self::Visual => "表示・視界",
            Self::Hud => "HUD",
            Self::Other => "その他・パッチ",
        }
    }
}
pub(super) fn category(name: &str) -> Category {
    match name {
        "autosprint" | "autowalk" | "autosneak" | "autojump" | "autoswim" | "inventorymove"
        | "fly" | "timer" | "safewalk" => Category::Movement,
        // Attacking, its targets, and awareness of other players.
        "autoclicker" | "killaura" | "triggerbot" | "autoweapon" | "friends" | "combathud"
        | "hitmarker" | "targethud" | "playeralert" => Category::Combat,
        // The world around the player: automation in it, places, maps, its
        // inhabitants and environment.
        "autotool" | "autoeat" | "autofish" | "durabilityguard" | "waila" | "waypoints"
        | "deathpoint" | "chunkborder" | "lightoverlay" | "minimap" | "worldmap" | "worldinfo"
        | "mobcount" | "playerlist" => Category::World,
        "esp" | "blockesp" | "blockoutline" | "nametags" | "breadcrumbs" | "zoom" | "fov"
        | "fullbright" | "crosshair" | "trajectories" | "freelook" | "freecam" => Category::Visual,
        // On-screen readouts of the player, the inventory and the client.
        "hud" | "effects" | "air" | "armorhud" | "inventoryhud" | "mininghud" | "itemcounter"
        | "durabilityalert" | "sessionstats" | "lowhealth" | "vitals" | "radar" | "compass"
        | "coordinates" | "fps" | "cps" | "speed" | "direction" | "dimension" | "clock"
        | "stopwatch" | "memory" | "entities" | "server" | "keystrokes" | "frametime"
        | "chathud" | "debughud" | "ping" => Category::Hud,
        // Tools that are none of the above: chat logging/sending, notices,
        // exports and diagnostics. User patches also land here.
        "chatlog" | "chatrepeat" | "joinnotify" | "skinstealer" | "performanceprofiler" => {
            Category::Other
        }
        _ => Category::Other,
    }
}
fn name_cmp(a: &str, b: &str) -> Ordering {
    a.bytes()
        .map(|b| b.to_ascii_lowercase())
        .cmp(b.bytes().map(|b| b.to_ascii_lowercase()))
}

#[derive(Default)]
pub(super) struct FeatureList {
    pub sort: Sort,
    pub category_filter: Option<Category>,
    pub order: Vec<usize>,
}
impl FeatureList {
    /// Borrow only names/states: sorting never constructs status strings or
    /// changes module identity, hotkeys, profile order, or native execution order.
    pub fn update<'a>(
        &mut self,
        count: usize,
        entry: impl Fn(usize) -> (&'a str, bool),
        filter: &str,
    ) {
        let filter = filter.trim().as_bytes();
        let selected_category = self.category_filter;
        self.order.clear();
        self.order.extend((0..count).filter(|&i| {
            if selected_category.is_some_and(|c| category(entry(i).0) != c) {
                return false;
            }
            let contains = |name: &str| {
                name.as_bytes()
                    .windows(filter.len())
                    .any(|s| s.eq_ignore_ascii_case(filter))
            };
            filter.is_empty()
                || contains(entry(i).0)
                || contains(crate::localization::module_name(entry(i).0))
                || contains(crate::localization::module_search_alias(entry(i).0))
        }));
        self.order.sort_unstable_by(|&a, &b| {
            let (an, ae) = entry(a);
            let (bn, be) = entry(b);
            let names = || name_cmp(an, bn);
            let cmp = match self.sort {
                Sort::Default => a.cmp(&b),
                Sort::Name => names(),
                Sort::NameDescending => names().reverse(),
                Sort::Enabled => be.cmp(&ae).then_with(names),
                Sort::Category => category(an).cmp(&category(bn)).then_with(names),
            };
            cmp.then_with(|| a.cmp(&b))
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_other_category_holds_the_tools() {
        for name in [
            "chatlog",
            "chatrepeat",
            "joinnotify",
            "skinstealer",
            "performanceprofiler",
        ] {
            assert!(category(name) == Category::Other, "{name}");
        }
        // On-screen panels stay with the HUD.
        assert!(category("chathud") == Category::Hud);
        assert!(category("debughud") == Category::Hud);
        assert!(category("ping") == Category::Hud);
    }

    #[test]
    fn world_and_combat_hold_their_features_instead_of_the_hud() {
        for name in [
            "minimap",
            "worldmap",
            "worldinfo",
            "mobcount",
            "playerlist",
            "lightoverlay",
        ] {
            assert!(category(name) == Category::World, "{name}");
        }
        for name in ["targethud", "playeralert"] {
            assert!(category(name) == Category::Combat, "{name}");
        }
    }

    #[test]
    fn category_filters_compose_with_search_and_keep_identity() {
        let entries = [
            ("autofish", false),
            ("autowalk", true),
            ("armorhud", true),
            ("autotool", true),
        ];
        let mut list = FeatureList {
            category_filter: Some(Category::World),
            sort: Sort::Enabled,
            ..Default::default()
        };
        list.update(entries.len(), |i| entries[i], "自動");
        assert_eq!(list.order, [3, 0]);
        list.category_filter = Some(Category::Hud);
        list.update(entries.len(), |i| entries[i], "自動");
        assert!(list.order.is_empty());
        list.update(entries.len(), |i| entries[i], "");
        assert_eq!(list.order, [2]);
        list.category_filter = None;
        list.update(entries.len(), |i| entries[i], "autofish");
        assert_eq!(list.order, [0]);
    }

    #[test]
    fn sorting_filters_and_preserves_original_module_identity() {
        let entries = [
            ("zoom", false),
            ("autowalk", true),
            ("air", false),
            ("Air", true),
        ];
        let mut list = FeatureList::default();
        for (sort, expected) in [
            (Sort::Default, vec![0, 1, 2, 3]),
            (Sort::Name, vec![2, 3, 1, 0]),
            (Sort::NameDescending, vec![0, 1, 2, 3]),
            (Sort::Enabled, vec![3, 1, 2, 0]),
        ] {
            list.sort = sort;
            list.update(entries.len(), |i| entries[i], "");
            assert_eq!(list.order, expected);
        }
        list.update(entries.len(), |i| entries[i], " AiR ");
        assert_eq!(list.order, [3, 2]);
        list.update(entries.len(), |i| entries[i], "not present");
        assert!(list.order.is_empty());
        list.update(0, |_| unreachable!(), "");
        assert!(list.order.is_empty());
    }

    #[test]
    fn japanese_search_keeps_the_original_command_indices() {
        let entries = [("autofish", false), ("autowalk", false), ("armorhud", true)];
        let mut list = FeatureList::default();
        list.update(entries.len(), |i| entries[i], "自動");
        assert_eq!(list.order, [0, 1]);
        list.update(entries.len(), |i| entries[i], "  防具  ");
        assert_eq!(list.order, [2]);
        list.update(entries.len(), |i| entries[i], "AuToFiSh");
        assert_eq!(list.order, [0]);
    }

    #[test]
    fn category_sort_handles_patches_and_refreshes_enabled_states() {
        let mut entries = [
            ("my patch", false),
            ("air", false),
            ("zoom", false),
            ("autowalk", false),
            ("killaura", false),
            ("autotool", false),
        ];
        let mut list = FeatureList {
            sort: Sort::Category,
            ..Default::default()
        };
        list.update(entries.len(), |i| entries[i], "");
        assert_eq!(list.order, [3, 4, 5, 2, 1, 0]);
        list.sort = Sort::Enabled;
        entries[0].1 = true;
        list.update(entries.len(), |i| entries[i], "");
        assert_eq!(list.order[0], 0);
        entries[0].1 = false;
        entries[2].1 = true;
        list.update(entries.len(), |i| entries[i], "");
        assert_eq!(list.order[0], 2);
    }
}
