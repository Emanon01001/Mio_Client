//! Typed menu settings shared by module controls and profile validation.
use crate::input;
use serde::{Deserialize, Serialize};
pub(crate) mod key_capture;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Value {
    Bool(bool),
    Number(f64),
    Key(String),
    /// A distinct table preserves untagged legacy key strings in profiles.
    Text {
        text: String,
    },
}
impl Value {
    pub fn boolean(&self) -> Result<bool, String> {
        if let Self::Bool(v) = self {
            Ok(*v)
        } else {
            Err("ON/OFFの値を指定してください".into())
        }
    }
    pub fn number(&self) -> Result<f64, String> {
        if let Self::Number(v) = self {
            Ok(*v)
        } else {
            Err("数値を指定してください".into())
        }
    }
    pub fn key(&self) -> Result<u16, String> {
        if let Self::Key(v) = self {
            input::parse_key(v).ok_or_else(|| format!("不明なキー: {v}"))
        } else {
            Err("キー名を指定してください".into())
        }
    }
    pub fn text(&self) -> Result<&str, String> {
        match self {
            Self::Text { text } => Ok(text),
            _ => Err("文字列を指定してください".into()),
        }
    }
    pub fn valid(&self) -> bool {
        match self {
            Self::Bool(_) => true,
            Self::Number(v) => v.is_finite(),
            Self::Key(v) => v.len() <= 32 && input::parse_key(v).is_some(),
            Self::Text { text } => text.len() <= 4096 && !text.contains('\0'),
        }
    }
}
pub enum Kind {
    Bool(bool),
    Number {
        value: f64,
        min: f64,
        max: f64,
        integer: bool,
    },
    Key(u16),
    /// Edited by the module's dedicated controls; included in profile snapshots.
    Text(String),
}
pub struct Detail {
    pub id: &'static str,
    pub label: &'static str,
    pub hint: &'static str,
    pub kind: Kind,
}
impl Detail {
    pub fn x_offset(value: i32, hint: &'static str) -> Self {
        Self::integer("x", "X offset", hint, value, -7680, 7680)
    }

    pub fn y_offset(value: i32, hint: &'static str) -> Self {
        Self::integer("y", "Y offset", hint, value, -4320, 4320)
    }

    pub fn text(id: &'static str, value: String) -> Self {
        Self {
            id,
            label: "",
            hint: "",
            kind: Kind::Text(value),
        }
    }
    pub fn boolean(id: &'static str, label: &'static str, hint: &'static str, value: bool) -> Self {
        Self {
            id,
            label: crate::localization::text(label),
            hint: crate::localization::text(hint),
            kind: Kind::Bool(value),
        }
    }
    pub fn number(
        id: &'static str,
        label: &'static str,
        hint: &'static str,
        value: f64,
        min: f64,
        max: f64,
    ) -> Self {
        Self {
            id,
            label: crate::localization::text(label),
            hint: crate::localization::text(hint),
            kind: Kind::Number {
                value,
                min,
                max,
                integer: false,
            },
        }
    }
    pub fn integer(
        id: &'static str,
        label: &'static str,
        hint: &'static str,
        value: i32,
        min: i32,
        max: i32,
    ) -> Self {
        Self {
            id,
            label: crate::localization::text(label),
            hint: crate::localization::text(hint),
            kind: Kind::Number {
                value: value as f64,
                min: min as f64,
                max: max as f64,
                integer: true,
            },
        }
    }
    pub fn key(id: &'static str, label: &'static str, hint: &'static str, key: u16) -> Self {
        Self {
            id,
            label: crate::localization::text(label),
            hint: crate::localization::text(hint),
            kind: Kind::Key(key),
        }
    }
    pub fn value(&self) -> Value {
        match self.kind {
            Kind::Bool(v) => Value::Bool(v),
            Kind::Number { value, .. } => Value::Number(value),
            Kind::Key(v) => Value::Key(input::key_name(v)),
            Kind::Text(ref text) => Value::Text { text: text.clone() },
        }
    }
    pub fn validate(&self, value: &Value) -> Result<(), String> {
        if !value.valid() {
            return Err(format!("{}: 不正な値です", self.id));
        }
        match self.kind {
            Kind::Bool(_) => {
                value.boolean()?;
            }
            Kind::Key(_) => {
                value.key()?;
            }
            Kind::Text(_) => {
                value.text()?;
            }
            Kind::Number {
                min, max, integer, ..
            } => {
                let n = value.number()?;
                if !(min..=max).contains(&n) || (integer && n.fract() != 0.) {
                    return Err(format!(
                        "{}: {}{}～{}の範囲で指定してください",
                        self.id,
                        if integer { "整数 " } else { "" },
                        min,
                        max
                    ));
                }
            }
        }
        Ok(())
    }
}

/// Choose from the list or capture a fresh physical key press.
pub fn key_control(ui: &hudhook::imgui::Ui, label: &str, key: u16) -> Option<u16> {
    let mut selected = None;
    let display_key = |key| {
        if key == 0 {
            "未割り当て".to_string()
        } else {
            input::display_key_name(key)
        }
    };
    let _id = ui.push_id(label);
    ui.text_colored(crate::theme::MUTED, crate::localization::text(label));
    ui.set_next_item_width(-1.);
    if let Some(_combo) = ui.begin_combo("##key", display_key(key)) {
        let mut keys: Vec<u16> = (0..=255).collect();
        keys.sort_by_key(|vk| match vk {
            0 => 0,
            0x70..=0x87 => 1,
            0x41..=0x5A => 2,
            0x30..=0x39 => 3,
            0x05 | 0x06 | 0x09 | 0x20..=0x24 | 0x2D | 0x2E | 0xA0..=0xA5 => 4,
            _ => 5,
        });
        for vk in keys {
            if ui
                .selectable_config(display_key(vk))
                .selected(vk == key)
                .build()
            {
                selected = Some(vk);
            }
            if vk == key {
                ui.set_item_default_focus();
            }
        }
    }
    key_capture::control(ui).or(selected)
}

/// A number set with a slider, or typed into the box beside it. A typed
/// value is clamped to `min..=max` (and rounded when `integer`); ImGui keeps
/// the text being typed, so applying each keystroke does not disturb it.
/// Returns the new value when either control changes it.
pub fn number_input(
    ui: &hudhook::imgui::Ui,
    id: &str,
    value: f64,
    min: f64,
    max: f64,
    integer: bool,
    format: &str,
) -> Option<f64> {
    let _id = ui.push_id(id);
    let spacing = ui.clone_style().item_spacing[0];
    let box_width = (ui.calc_text_size("-00000.000")[0] + 16.).max(70.);
    let slider_width = (ui.content_region_avail()[0] - box_width - spacing).max(60.);
    let mut slid = value;
    ui.set_next_item_width(slider_width);
    let mut changed = ui
        .slider_config("##slider", min, max)
        .display_format(format)
        .build(&mut slid)
        .then_some(slid);
    ui.same_line();
    ui.set_next_item_width(box_width);
    let mut typed = value;
    if ui
        .input_scalar("##input", &mut typed)
        .display_format(format)
        .build()
        && typed.is_finite()
    {
        changed = Some(typed);
    }
    changed.map(|v| {
        let v = v.clamp(min, max);
        if integer {
            v.round()
        } else {
            v
        }
    })
}

pub fn control(ui: &hudhook::imgui::Ui, detail: &Detail) -> Option<Value> {
    let _id = ui.push_id(detail.id);
    let changed = match detail.kind {
        Kind::Bool(mut v) => ui.checkbox(detail.label, &mut v).then_some(Value::Bool(v)),
        Kind::Key(v) => key_control(ui, detail.label, v).map(|k| Value::Key(input::key_name(k))),
        Kind::Text(_) => None,
        Kind::Number {
            value,
            min,
            max,
            integer,
        } => {
            ui.text_colored(crate::theme::MUTED, detail.label);
            let format = if integer {
                "%.0f"
            } else if min > 0. && min < 0.01 {
                // Keep sub-centisecond values visible, e.g. ChatRepeat's 1ms.
                "%.3f"
            } else {
                "%.2f"
            };
            number_input(ui, "##value", value, min, max, integer, format).map(Value::Number)
        }
    };
    if !detail.hint.is_empty() && ui.is_item_hovered() {
        ui.tooltip_text(detail.hint);
    }
    changed
}

pub fn description(name: &str) -> &'static str {
    match name {
        "hud" => "有効な機能と数値をまとめて表示します。HUD編集か横・縦位置で移動できます。負の値は右端・下端を基準にします。",
        "autosprint" => "前進中に通常のダッシュ入力を加えます。接地中だけ自動ジャンプする設定もあります。",
        "safewalk" => "足元の支えがなくなる付近でスニークします。手動ジャンプを優先し、部分ブロックや未確認の形状では安全側に判定します。",
        "durabilityguard" => "残り耐久度が設定値を下回ると同系統の道具へ切り替えます。ホットバーを優先し、予備が通常インベントリにある場合は手持ちスロットと交換します。食事・釣りなどの使用を優先します。",
        "inventorymove" => "インベントリを開いたまま移動できます。チャット・ポーズ・クライアントのメニューでは停止し、戦闘機能は継続しません。",
        "autowalk" => "キーを押さずに前進します。自動ダッシュと併用でき、後退入力で一時停止できます。",
        "autosneak" => "通常のスニーク入力を保持します。OFFにしても手動のスニーク入力は維持します。",
        "autojump" => "ダッシュせずに接地時だけジャンプします。スニーク中は自動ジャンプを停止します。",
        "autoswim" => "頭が水中にある間だけ浮上します。スニーク操作で自動浮上を止めて潜れます。",
        "autoclicker" => "マウスの左ボタンを押している間、通常の攻撃処理を繰り返します。",
        "killaura" => "近くの対象を自動攻撃します。メニューを開いている間は停止します。",
        "triggerbot" => "照準に当たる最も手前のエンティティだけを自動攻撃します。ブロックや対象外のエンティティに遮られた相手は攻撃せず、メニュー・インベントリでは停止します。KillAuraが優先されます。",
        "autoweapon" => "エンティティへの攻撃直前に、ホットバーの剣へ切り替えます。通常素材の強さを比較し、エンチャントは比較しません。切替後はそのスロットを維持します。",
        "friends" => "登録した表示名のプレイヤーを自動攻撃の対象から外します。手動の単発攻撃は可能です。登録内容はプロファイルに保存できます。",
        "esp" => "エンティティの当たり判定を枠で表示します。壁の向こうの対象も表示します。",
        "blockesp" => "読み込み済みの範囲にある鉱石・チェスト・シュルカーボックス・指定したブロックを囲みます。コマンドは blocks add <ID> です。",
        "radar" => "周囲のエンティティを、視点の前方が上になる地図で表示します。プレイヤーは緑、敵対モブは赤、中立モブは黄、友好モブは青です。負の横・縦位置は右端・下端が基準です。",
        "targethud" => "現在の自動攻撃対象を表示します。負の横・縦位置は画面右端・下端が基準です。",
        "combathud" => "直前に攻撃した対象（名前・体力バー・距離）と、このセッションのヒット数・撃破数・コンボ・周囲の敵数を表示します。攻撃の判定はエンティティの体力の減少から行い（約100ms間隔）、照準の先・射程内で減った対象を自分のヒットとみなします。混戦時は取りこぼすことがあります。",
        "hitmarker" => "攻撃が当たった瞬間に照準へXを表示します（撃破時は赤く大きく）。判定は戦闘HUDと同じ体力の減少によります。",
        "vitals" => "体力・衝撃吸収・空腹値・隠し満腹度・消耗度・経験値を表示します。",
        "nametags" => "プレイヤー名やカスタム名、体力、距離を当たり判定の上に表示します。表示名がない場合は種類名を使います。",
        "blockoutline" => "照準が当たるブロックを色付きの枠で強調します。色・透明度・太さを変更できます。",
        "playerlist" => "表示キーを押している間、ワールドの参加者を名前順で表示します。読み込み済みのプレイヤーは距離・体力を取得できます。人数が多い場合は左右キーでページを切り替えます。",
        "effects" => "ステータス効果の名前・強さ・ゲーム内の残り時間を表示します。終了前の通知も設定できます。",
        "air" => "現在の酸素量を表示します。溺れるまでの推定時間ではなく、ゲーム内の実際のカウンターを使います。",
        "crosshair" => "ゲーム標準の照準に重ねて描きます。大きさ・隙間・色・移動時の広がりは見た目だけの設定です。",
        "lowhealth" => "体力が指定の割合以下になると通知します。設定値より5ポイント上まで回復する間、赤い枠を表示します。",
        "playeralert" => "近くのプレイヤー数を表示し、範囲に入ると通知します。読み込み済みのエンティティだけが対象です。",
        "compass" => "視点の方向を中心にした方位バーを表示します。",
        "keystrokes" => "実際の移動キーとマウスボタンの状態を表示します。負の横・縦位置は右端・下端が基準です。",
        "coordinates" => "HUDに座標を表示します。Minecraft標準の座標表示を有効にすることもできます。",
        "inventoryhud" => "所持品を9列のスロットにアイコン・個数・耐久バーで表示します。通常インベントリが上3段、ホットバーが最下段です。文字一覧にも切り替えられ、位置はHUD編集で変更できます。",
        "armorhud" => "装備している防具4部位の名前と残り耐久を文字で表示します。",
        "durabilityalert" => "ホットバーの道具や装備中の防具の残り耐久が指定の割合以下になると通知します。修理後は再び通知できます。",
        "autofish" => "自分の浮きのアタリを検出して回収します。最初は手動で投げてください。自分のインベントリ中も継続し、浮きが消えてから再投します。持ち替え・ワールドでの手動右クリック・他のメニューでは停止します。",
        "zoom" => "指定キーを押している間、ワールドの表示を拡大します。",
        "fullbright" => "ゲームの描画処理に返す明るさを調整します。",
        "timer" => "ゲームの時計の進み方を変えます。1より大きい値で速くなります。",
        "fly" => "指定した速度で飛行します。",
        "waypoints" => "地点を保存し、追加したディメンションで表示します。コマンド: wp add <名前>、wp list、wp del <名前>。",
        "chunkborder" => "現在のチャンクの境界を表示します。",
        "fov" => "ワールドの視野角を30～170度に固定し、ダッシュや速度による視野角の変化を置き換えます。",
        "waila" => "照準の線が当たる最も手前のブロックやエンティティを表示します。名前・座標・距離・取得できる体力を表示し、ブロックの向こうのエンティティは表示しません。距離はブロック単位です。",
        "mininghud" => "実際に採掘しているブロック名と、ゲーム内の破壊進捗を表示します。進捗はゲームから読み取った値で、推定時間ではありません。",
        "itemcounter" => "指定したアイテムと手持ちアイテムの合計所持数を表示します。通常インベントリとホットバーが対象で、チェストや防具は含みません。",
        "breadcrumbs" => "有効にしてから歩いた経路を線で表示します。",
        "dimension" => "現在のディメンションを表示します。",
        "stopwatch" => "有効にした時点からの経過時間を表示します。",
        "ping" => "接続中のサーバーとの往復時間をmsで表示します。ゲーム自身の通信（RakNetの接続中Pingの応答とACK、NetherNetの接続確認STUN）を読み取り専用で観測し、こちらからは送信しません。取得できるまでの間は、サーバー一覧用Pingで補います。初期OFFです。",
        "chatlog" => "時刻付きチャットを保存し、登録語を含む受信を強調・通知します。メニュー中の検索画面では本文や送信者を検索し、結果をコピーできます。",
        "chatrepeat" => "指定した本文を一定間隔で通常チャットへ送信します。本文・間隔・回数を設定し、Detailsの開始ボタンで実行します。チャット表示中も継続し、入力中の本文には触れません。メニュー・インベントリ・非アクティブ時は一時停止します。",
        "chathud" => "直近のチャットを時刻付きでパネルに常時表示します。送信者ごとに色分けし、同じ発言の連投は「×3」のようにまとめます。HUD編集で移動できます。",
        "joinnotify" => "「〜がゲームに参加しました」などのシステムメッセージから入退室を検出して通知し、このセッションの履歴を詳細設定に表示します。",
        "worldinfo" => "現在のディメンションのゲーム内時刻と、雨・雷の強さを表示します。時計・天気は読み取り専用です。",
        "mobcount" => "読み込み済みの生きているMobを種類別に数え、多い順に表示します。プレイヤーや落ちているアイテムは含みません。",
        "lightoverlay" => "近くの床の実際の明るさを読み取ります。赤は現在暗い床、黄は空の光がなくなると暗い床です。湧き潰しの目安で、Mob出現の保証ではありません。",
        "freelook" => "見回しキーを押している間、体の向きを変えずにカメラを回します。離すと元の視点へ戻り、メニューやインベントリ中は解除します。",
        "freecam" => "プレイヤーを動かさず、カメラだけを自由に移動します。W/A/S/Dで視点方向へ、Spaceで上昇、Shiftで下降、Ctrlで加速します。ON中はプレイヤーへの移動・攻撃・使用入力を止め、OFFで通常視点へ戻ります。",
        "minimap" => "周囲の地形を上から見た地図で表示します。北が上で、矢印が自分の向きです。地下にいても常に地表のブロックを表示します。読み込み済みの範囲だけを読み取り、一度読んだ地形は記憶して暗く表示します。",
        "trajectories" => "弓・クロスボウ・トライデント・投げられるアイテムを持っている間、飛ぶ経路と当たるブロックやエンティティを表示します。弓は引いている長さを反映します。推定値で、実際の着弾とずれることがあります。",
        "frametime" => "直近5秒のフレーム時間をグラフで表示します。平均・最大・1% lowで、平均FPSでは分からないカクつきを確認できます。",
        "performanceprofiler" => "機能ごとの更新・描画、共有データ取得の経過時間を計測します。平均・最大・毎秒時間・回数・走査件数を表示します。初期OFF。GPU時間と全体CPU使用率は測定しません。",
        "debughud" => "F3で座標・チャンク内位置・向き・速度・FPS・メモリ・ディメンション・照準先を左右に表示します。表示中は機能一覧カードを一時的に隠します。Java版の完全な再現ではなく、既存の取得データを使う読み取り専用の画面です。右側の位置はDetailsで調整できます。",
        "worldmap" => "記憶した地形を全画面の地図で表示します（既定M）。ホイールで拡大縮小、ドラッグで移動、右クリックで現在地へ戻り、開いたキーをもう一度押すと閉じます（Escでは閉じません）。ワールドID・ディメンション別に定期保存・退出時保存し、次の入場時に自動復元します。自動保存は初期ONで、地図を閉じていても地形を記録します。",
        "skinstealer" => "読み込み済みプレイヤーを選んでスキン用PNGを保存します。対応PersonaはモデルのUVと顔画像から64×64へ変換します。独自の立体形状には制限があります。元テクスチャの保存も別ボタンで選べます。",
        _ => "機能のON/OFFと切り替えキーを設定します。",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn detail_validation_rejects_invalid_types_ranges_and_keys() {
        let position = Detail::integer("x", "X", "", 0, -100, 100);
        for v in [
            Value::Number(f64::NAN),
            Value::Number(f64::INFINITY),
            Value::Number(101.),
            Value::Number(1.5),
            Value::Bool(true),
        ] {
            assert!(position.validate(&v).is_err());
        }
        assert!(position.validate(&Value::Number(-100.)).is_ok());
        let flag = Detail::boolean("jump", "Jump", "", false);
        assert!(flag.validate(&Value::Number(1.)).is_err());
        assert!(flag.validate(&Value::Bool(true)).is_ok());
        let key = Detail::key("key", "Key", "", 0);
        for name in ["F25", "0x100", "bogus"] {
            assert!(key.validate(&Value::Key(name.into())).is_err());
        }
        assert!(key.validate(&Value::Key("F6".into())).is_ok());
        assert!(key.validate(&Value::Key("NONE".into())).is_ok());
    }
}
