//! Minecraft Bedrock 1.26.6301.0用の組み込みシグネチャ一覧（修正版1.26.65.1でも全件同じ数だけ一致）。
//!
//! 各コメントに検出対象と利用機能を記載。`??`は変動するアドレス・分岐距離などのバイト。
//! パターンの一致だけで他バージョンへの対応は保証されない。ABI・オフセットの検証も必要。
//! ユーザー指定の追加パッチは従来どおり設定ファイルのsignatureを使用する。

/// SkinStealer: スキンIDのMSVC文字列。length/capacity/data/inline参照の4箇所から同じフィールドを確認する。
pub(crate) const SKIN_ID_FIELD_SIGNATURE: &str = "48 8B 41 ?? 48 83 79 ?? 10 72 ?? 48 8B 49 ?? 4C 39 C0 74 ?? EB ?? 66 90 48 83 C1 ?? 4C 39 C0 75 ??";

/// SkinStealer: PersonaのGeometryData（SerializedSkin+0xF8）をJSONへ渡す反射getter。座標・UVを読む根拠で、関数は呼び出さない。
pub(crate) const SKIN_GEOMETRY_FIELD_SIGNATURE: &str = "41 B8 ?? ?? ?? ?? 4C 03 07 48 8D 4D C8 48 8D 55 A8 E8 ?? ?? ?? ?? 90 48 8D 05 ?? ?? ?? ?? 48 89 45 C8 48 8B 45 E8 48 83 F8 10";
/// SkinStealer: AnimatedImageData配列（SerializedSkin+0xE0、反射型8AF9AA5F）。顔は本体アトラスと別画像のため、変換時だけコピーする。
pub(crate) const SKIN_ANIMATION_FIELD_SIGNATURE: &str = "B8 ?? ?? ?? ?? 48 03 07 48 8B 4B 28 48 8D 15 ?? ?? ?? ?? 48 89 56 10 C7 46 20 5F AA F9 8A 48 C7 46 18 00 00 00 00 C6 46 24 04";

/// SkinStealer: Actor+0xAD8のSerializedSkinRefへのshared_ptr代入を照合する（実行しない）。
pub(crate) const ACTOR_SKIN_REF_SIGNATURE: &str = "55 41 57 41 56 41 55 41 54 56 57 53 48 81 EC F8 00 00 00 48 8D AC 24 80 00 00 00 0F 29 75 60 48 C7 45 58 FE FF FF FF 44 89 C3 48 89 D7 48 89 CE 48 8B 81 ?? ?? ?? ?? 48 8B 4A 08 48 85 C9 74 0A F0 FF 41 08 48 8B 4F 08 EB 02 31 C9 48 8B 17 48 89 10 4C 8B 70 08 48 89 48 08";

/// SkinStealer: ClientSkinプールの解析用アンカー（実プレイヤーのIDとの直接照合には使用しない）。
pub(crate) const CLIENT_SKIN_SIGNATURE: &str = "55 41 57 41 56 41 55 41 54 56 57 53 48 81 EC 98 00 00 00 48 8D AC 24 80 00 00 00 48 C7 45 10 FE FF FF FF 48 89 D6 49 89 CE 48 89 4D F0 48 83 C1 30 BA AB 13 B2 83 E8 ?? ?? ?? ?? 48 89 C7 48 89 45 F8 C7 45 00 FF FF FF FF 48 8B 48 20";
/// SkinStealer: SerializedSkinのImageData（+0x80、反射型D1E27925）。マントやアニメ画像と区別。
pub(crate) const SKIN_IMAGE_FIELD_SIGNATURE: &str = "48 8B 07 48 83 E8 ?? 48 8B 4B 28 48 8D 15 ?? ?? ?? ?? 48 89 56 10 C7 46 20 25 79 E2 D1 48 C7 46 18 00 00 00 00";
/// SkinStealer: ImageDataのnativeコピー。画像ヘッダー+0x80・Blobデータ+0xA0/長さ+0xA8を照合。書き込み関数は呼ばない。
pub(crate) const SKIN_IMAGE_COPY_SIGNATURE: &str = "55 56 57 53 48 83 EC 58 48 8D 6C 24 50 48 C7 45 00 FE FF FF FF 48 89 D0 48 8B 31 48 8D 55 D0 48 89 C1 E8 ?? ?? ?? ?? 48 8B 45 F0 0F 57 C0 48 8B 7D F8 0F 11 45 F0 48 8B 5D E8 0F B7 4D E0 66 89 8E ?? ?? ?? ?? 0F 10 45 D0 0F 11 86 ?? ?? ?? ?? 48 8B 8E ?? ?? ?? ?? 48 89 86 ?? ?? ?? ?? 48 85 C9 74 0E 48 8B 86 ?? ?? ?? ?? FF 15 ?? ?? ?? ?? 90 48 89 9E ?? ?? ?? ?? 48 89 BE ?? ?? ?? ??";

/// DebugHUD: FMODの初期化（32チャンネル、flags=4）。thread-unsafe初期化を使用していないことを照合。
pub(crate) const SOUND_INIT_SIGNATURE: &str = "48 83 EC 28 48 8B 49 50 BA 20 00 00 00 41 B8 04 00 00 00 45 31 C9 E8 ?? ?? ?? ?? 85 C0 0F 94 C0 48 83 C4 28 C3";
/// DebugHUD: Levelの保存フォルダーID文字列+0x260の参照取得。
pub(crate) const LEVEL_ID_FIELD_SIGNATURE: &str =
    "48 8D 81 60 02 00 00 C3 CC CC CC CC CC CC CC CC 48 8D 81 E0 02 00 00 C3";
/// DebugHUD: Biomeの識別名+0x198の参照取得。
pub(crate) const BIOME_NAME_FIELD_SIGNATURE: &str =
    "48 8D 81 98 01 00 00 C3 CC CC CC CC CC CC CC CC 48 8D 81 B8 01 00 00 C3";
/// DebugHUD: ロード済みBlockSourceの座標からBiomeを取得（ABI: source, BlockPos* -> Biome*）。
pub(crate) const GET_BIOME_SIGNATURE: &str = "56 57 48 83 EC 28 48 89 D7 48 89 CE 48 8B 01 48 8B 80 40 01 00 00 FF 15 ?? ?? ?? ?? 48 85 C0 74 49 8B 4F 04 0F BF 56 3A";
/// DebugHUD: 水没ブロックの副層+0x38を取得。通常のgetBlock（主層+0x30）と区別。
pub(crate) const GET_EXTRA_BLOCK_SIGNATURE: &str = "56 57 48 83 EC 38 48 89 CF 48 8B 05 ?? ?? ?? ?? 48 31 E0 48 89 44 24 30 8B 42 04 0F BF 49 3A 39 C8 0F 8C ?? ?? ?? ?? 0F BF 4F 38 39 C8 0F 8D ?? ?? ?? ?? 48 89 D6 8B 02 C1 F8 04 89 44 24 28 8B 42 08 C1 F8 04 89 44 24 2C 48 8B 07 48 8B 80 48 01 00 00 48 8D 54 24 28 48 89 F9 FF 15 ?? ?? ?? ?? 48 85 C0 74 ?? 44 0F B7 46 04 66 44 2B 47 3A 41 0F BF C8 C1 E9 04 48 0F BF C9 48 8B 90 C8 01 00 00 48 8B 80 D0 01 00 00 48 29 D0 48 C1 F8 03 49 B9 C5 4E EC C4 4E EC C4 4E 4C 0F AF C8 49 39 C9 76 ?? 48 6B C1 68 48 8B 4C 02 38 48 85 C9 74 ?? 0F B7 46 08";
/// DebugHUD: FMOD SoundSystemの統計処理。vtable+0x210の照合だけに使い、文字列生成は呼ばない。
pub(crate) const SOUND_DEBUG_SIGNATURE: &str = "55 41 57 41 56 41 55 41 54 56 57 53 48 81 EC 18 01 00 00 48 8D AC 24 80 00 00 00 48 C7 85 90 00 00 00 FE FF FF FF 48 89 D6 48 89 CF 48 8B 42 10 48 8B 4A 18 48 89 CA 48 29 C2 48 83 FA 11 73 73 48 C7 44 24 20 11 00 00 00";
/// DebugHUD: SoundEngine+0x18からSoundSystemの統計処理へ委譲する参照経路。
pub(crate) const SOUND_ENGINE_STATS_SIGNATURE: &str =
    "48 8B 49 18 48 8B 01 48 8B 80 10 02 00 00 4C 8B 05 ?? ?? ?? ?? 49 FF E0";

/// WorldInfo: Level::getTime。vtable+0x420と照合し、+0x5CのLEAから標準WorldClockキーを取得。直接呼ばない。
pub(crate) const LEVEL_TIME_SIGNATURE: &str = "55 56 57 48 83 EC 40 48 8D 6C 24 40 48 C7 45 F8 FE FF FF FF 48 8B 01 48 8B 80 90 0C 00 00 FF 15 ?? ?? ?? ?? 48 8B 08 4C 8B 41 18 48 8D 55 E0 48 89 C1 4C 89 C0 FF 15 ?? ?? ?? ??";
/// Level::getLevelData。弱参照の生存フラグ+0x80とデータ+0x90を照合し、呼び出さず読み取る。
pub(crate) const LEVEL_DATA_SIGNATURE: &str =
    "48 8B 81 80 00 00 00 48 85 C0 74 0D 80 38 00 74 18 48 8B 81 90 00 00 00 C3";
/// LevelNameタグの保存先。LevelData +0x2B0のMSVC文字列をDebugHUDのワールド名に使用。
pub(crate) const LEVEL_NAME_FIELD_SIGNATURE: &str =
    "4C 8D 48 08 48 85 C0 48 8D 05 ?? ?? ?? ?? 4C 0F 44 C8 4C 8D B6 B0 02 00 00";
/// DebugHUD: Level::getDifficulty。LevelData+0x3C0の0～3を読み、変更関数は呼ばない。
pub(crate) const LEVEL_DIFFICULTY_SIGNATURE: &str =
    "48 83 EC 28 48 8B 01 48 8B 80 60 05 00 00 FF 15 ?? ?? ?? ?? 8B 80 C0 03 00 00 48 83 C4 28 C3";
/// WorldInfo: WorldClockManagerの有効な参照の取得。+0x50/+0x58のレイアウトを照合する。
pub(crate) const CLOCK_HANDLE_SIGNATURE: &str = "56 57 53 48 83 EC 20 0F 57 C0 0F 11 02 48 C7 42 10 00 00 00 00 48 8B 41 50 48 85 C0 0F 84 ?? ?? ?? ?? 4C 8B 40 10 4D 85 C0";
/// WorldInfo: rainLevelコマンドのLevelData+0x374とWeather+0x3Cへの同期箇所。書き込まず読み取りの根拠だけに使う。
pub(crate) const WEATHER_FIELDS_SIGNATURE: &str =
    "F3 41 0F 11 86 74 03 00 00 41 89 86 78 03 00 00 F3 41 0F 11 44 24 3C 75 06";
/// LightOverlay: BlockSourceの明るさ取得。ABI(source, out:u8*, pos, skyDarken:u8, applyDarken:bool) -> u8*。
pub(crate) const BLOCK_LIGHT_SIGNATURE: &str = "41 56 56 57 53 48 83 EC 38 44 89 CB 4C 89 C7 48 89 D6 49 89 CE 48 8B 05 ?? ?? ?? ?? 48 31 E0 48 89 44 24 30 48 8B 01 48 8B 40 10 4C 89 C2 FF 15 ?? ?? ?? ?? 80 B8 A5 00 00 00 0F";
/// FreeLook: カメラのquaternion(+0x30)からローカルActorの向きを更新する処理。ABI(camera, perspective:u32*, Actor*)。
pub(crate) const CAMERA_PLAYER_SYNC_SIGNATURE: &str = "41 57 41 56 41 55 41 54 56 57 55 53 48 81 EC D8 00 00 00 44 0F 29 94 24 C0 00 00 00 44 0F 29 8C 24 B0 00 00 00 44 0F 29 84 24 A0 00 00 00 0F 29 BC 24 90 00 00 00 0F 29 B4 24 80 00 00 00 4C 89 C6 49 8B 40 10 45 8B 60 18";
/// Freecam: CameraComponent同士のnativeコピー。位置+0x40/44/48とquaternion+0x30/3Cを別々に照合。
/// 0x120バイトのCameraComponentプールを使う経路。フックせず配置検証にだけ使用する。
pub(crate) const CAMERA_POSITION_COPY_SIGNATURE: &str = "F3 0F 10 43 40 F3 0F 11 40 40 F3 0F 10 43 44 F3 0F 11 40 44 F3 0F 10 43 48 F3 0F 11 40 48 F3 0F 10 43 3C F3 0F 11 40 3C F3 0F 10 43 30 F3 0F 11 40 30";
/// Freecam: LevelRendererPlayerの描画カメラ設定。CameraComponent+0x40の位置を
/// 描画原点(+0x64C)と複製(+0x658)へ格納する箇所。オフセットはここから読み、
/// 関数先頭は直前のプロローグ(push r15/r14/r12/rsi/rdi/rbx; sub rsp)で特定する。
pub(crate) const RENDER_CAMERA_ORIGIN_SIGNATURE: &str = "F3 0F 10 43 48 F2 0F 10 4B 40 F2 0F 11 8E ?? ?? ?? ?? F3 0F 11 86 ?? ?? ?? ?? 8B 86 ?? ?? ?? ?? 89 86 ?? ?? ?? ?? 48 8B 86 ?? ?? ?? ?? 48 89 86 ?? ?? ?? ??";

/// AutoFish: FishingHook::handleActorEvent。自分の浮きのFISH_HOOK_TIMEを観測する。
/// 1.26.6301.0ではFishingHook vtable+0x220とも照合する。キー送信には使わない。
pub(crate) const FISH_EVENT_SIGNATURE: &str = "55 41 56 56 57 53 48 81 EC 70 01 00 00 48 8D AC 24 80 00 00 00 44 0F 29 8D E0 00 00 00 44 0F 29 85 D0 00 00 00 0F 29 BD C0 00 00 00 0F 29 B5 B0 00 00 00 48 C7 85 A8 00 00 00 FE FF FF FF 48 89 CE 80 FA 0E 0F 84 ?? ?? ?? ?? 0F B6 C2 83 F8 0D 0F 84 ?? ?? ?? ?? 83 F8 0C";

/// 耐久保護: 通常インベントリの交換をまとめるClientLegacyTransactionRequestの開始。
pub(crate) const INVENTORY_REQUEST_BEGIN_SIGNATURE: &str = "41 57 41 56 56 57 53 48 83 EC 20 48 89 D6 48 89 CF E8 ?? ?? ?? ?? 48 85 C0 74 ?? 48 89 C3 8B 05 ?? ?? ?? ?? 83 F8 FE B9 FE FF FF FF 0F 4C C8 83 C1 FE 89 0D ?? ?? ?? ?? 89 4F 50";
/// 耐久保護: ItemStack全体のnativeコピー。NBT/参照カウント/StackNetIdを保持する。
pub(crate) const INVENTORY_STACK_COPY_SIGNATURE: &str = "56 57 48 83 EC 28 48 89 D6 48 89 CF E8 ?? ?? ?? ?? 48 8D 05 ?? ?? ?? ?? 48 89 07 48 8D 87 80 00 00 00 C7 87 80 00 00 00 00 00 00 00 C6 87 90 00 00 00 00";
/// 耐久保護: クライアントInventory::setItem。予測・要求・変更通知を含む外側の経路。
/// 0x142B64940の内部setterだけを直接呼ばず、実際のコンテナvtable+0x68とも照合する。
pub(crate) const INVENTORY_SET_SIGNATURE: &str = "55 41 57 41 56 41 55 41 54 56 57 53 48 81 EC 78 04 00 00 48 8D AC 24 80 00 00 00 48 C7 85 F0 03 00 00 FE FF FF FF 85 D2 0F 88 ?? ?? ?? ?? 45 89 CF 4D 89 C4 41 89 D5 49 89 CE 48 8B 01 48 8B 80 A0 00 00 00 FF 15 ?? ?? ?? ?? 41 39 C5 0F 8D ?? ?? ?? ??";

/// Armor/Inventory HUD・AutoTool: ItemStackのNBT "Damage" を読む純粋な取得関数。
pub(crate) const ITEM_DAMAGE_SIGNATURE: &str = "56 57 48 83 EC 38 48 8B 05 ?? ?? ?? ?? 48 31 E0 48 89 44 24 30 48 8B 41 08 48 85 C0 74 ?? 48 83 38 00 74 ?? 48 8B 71 10 48 85 F6 74 ?? 48 8D 3D ?? ?? ?? ?? 48 89 7C 24 20 48 C7 44 24 28 06 00 00 00";
/// 装備の最大耐久値取得。Item vtable+0x120と一致することも検証する。
pub(crate) const ITEM_MAX_DAMAGE_SIGNATURE: &str = "0F B7 81 50 01 00 00 C3";
/// AutoFish/Item HUD: ComponentItemのminecraft:durability由来の最大耐久値。
/// 釣り竿などではItem+0x150は0のため、この検証済み仮想getterをゲームスレッドで呼ぶ。
pub(crate) const COMPONENT_MAX_DAMAGE_SIGNATURE: &str = "55 56 48 83 EC 28 48 8D 6C 24 20 48 C7 45 00 FE FF FF FF 48 89 CE 8B 05 ?? ?? ?? ?? 8B 0D ?? ?? ?? ?? 65 48 8B 14 25 58 00 00 00 48 8B 0C CA 3B 81 04 00 00 00 7F ?? 48 8B 06 48 8B 80 C0 00 00 00 48 8D 15 ?? ?? ?? ?? 48 89 F1 FF 15 ?? ?? ?? ?? 48 85 C0 74 ?? 0F B7 40 10 EB ?? 31 C0 48 83 C4 28 5E 5D C3";
/// Inventory HUD: 36スロット・152バイト要素のInventory::getItem。呼ばずvtable検証に使用。
pub(crate) const INVENTORY_ITEM_SIGNATURE: &str = "48 8D 05 ?? ?? ?? ?? 85 D2 78 2E 4C 8B 81 98 01 00 00 48 8B 89 A0 01 00 00 4C 29 C1 48 C1 E9 03 69 C9 1B CA 6B 28 39 CA 7D 0F 89 D0 48 69 C0 98 00 00 00 49 01 C0 4C 89 C0 C3";
/// Armor HUD: 装備コンテナのgetItem。+0x1A0の配列とスロット0分岐の種別を検証する。
pub(crate) const ARMOR_ITEM_SIGNATURE: &str = "85 D2 75 21 80 79 08 13 75 1B 48 8B 89 B8 01 00 00 48 8B 01 48 8B 80 68 02 00 00 48 8B 15 ?? ?? ?? ?? 48 FF E2 48 8D 05 ?? ?? ?? ?? 85 D2 78 2E 4C 8B 81 A0 01 00 00 48 8B 89 A8 01 00 00";
/// AutoTool/AutoEat: 通常のホットバー選択（ClientInstance*, 1始まりのスロット）。
pub(crate) const SELECT_SLOT_SIGNATURE: &str = "56 57 53 48 83 EC 40 89 D6 48 89 CB 48 8B 01 48 8B 80 F0 00 00 00 FF 15 ?? ?? ?? ?? 48 85 C0 0F 84 ?? ?? ?? ?? 48 89 C7 48 8B 03 48 8B 80 10 07 00 00";
/// AutoTool: 通常の入力処理が最後に呼ぶPlayerInventoryProxy::selectSlot。
/// (proxy*, 0始まりスロット, 0) -> bool。選択・同期処理をゲームに任せ、直接フィールドを書かない。
pub(crate) const PROXY_SELECT_SIGNATURE: &str = "55 41 57 41 56 41 55 41 54 56 57 53 48 81 EC 88 02 00 00 48 8D AC 24 80 00 00 00 48 C7 85 00 02 00 00 FE FF FF FF 45 84 C0 75 ?? 83 FA 08 77 ?? 8B 59 10 89 51 10 C6 81 B0 00 00 00 00 B0 01";
/// AutoEat: 通常の使用ボタン押下。RCX/RDX未使用、R8=ClientInstance。
pub(crate) const USE_DOWN_SIGNATURE: &str = "41 56 56 57 53 48 83 EC 28 4C 89 C6 49 8B 00 48 8B 80 D8 04 00 00 4C 89 C1 FF 15 ?? ?? ?? ?? 84 C0 0F 84 ?? ?? ?? ?? 48 8B 06 48 8B 80 B0 09 00 00 48 89 F1 BA B0 00 00 00";
/// AutoEat: 通常の使用ボタン解放。使用終了と入力状態の解除をゲームに任せる。
pub(crate) const USE_UP_SIGNATURE: &str = "56 57 53 48 83 EC 20 4C 89 C6 49 8B 00 48 8B 80 F0 00 00 00 4C 89 C1 FF 15 ?? ?? ?? ?? 48 85 C0 74 ?? 48 89 C7 48 8B 00 48 8B 80 68 02 00 00 48 89 F9 FF 15 ?? ?? ?? ?? 48 89 C1 E8 ?? ?? ?? ?? 89";
/// AutoFish: ClientInstance vtable+0x4D8の使用許可判定。
/// 自分のインベントリ内でAutoFishが通常のuseを呼ぶ間だけ、同一スレッド・同一CIに限定して許可する。
pub(crate) const USE_ALLOWED_SIGNATURE: &str = "55 56 57 53 48 83 EC 48 48 8D 6C 24 40 48 C7 45 00 FE FF FF FF 48 89 CE 48 8B 01 48 8B 80 00 05 00 00 FF 15 ?? ?? ?? ?? 84 C0 0F 84 ?? ?? ?? ?? 48 8B 06 48 8B 80 38 07 00 00 48 8D 55 E8";

/// 効果HUDのデータ配置検証。ゲーム自身が効果を列挙するループ。
/// +3=要素サイズ(0xA8)、+17=最大ID(38)、+36=強さフィールド(0x20)。フックしない。
pub(crate) const EFFECT_LAYOUT_SIGNATURE: &str = "48 81 C3 ?? ?? 00 00 4C 39 F3 74 ?? 8B 0B 48 83 F9 ?? 77 ?? 39 C1 74 ?? 49 8B 0C CF 48 85 C9 74 ?? 44 8B 43 ?? 48 8B 01 48 8B 40 28 F3 0F 11 74 24 20";

/// 名札と酸素HUDに使うDataItemのID・型アクセサ（隣接する2関数）。
/// +3=IDの位置(0x0A)、+19=型の位置(0x08)。取得関数は呼ばず、vtableの型確認に使う。
/// 別クラスの同形アクセサは0x20/0x22なので、位置の値を照合した後の一意性も要求する。
pub(crate) const METADATA_ACCESSORS_SIGNATURE: &str =
    "0F B7 41 ?? C3 CC CC CC CC CC CC CC CC CC CC CC 0F B6 41 ?? C3";

// 座標・プレイヤー取得

/// 座標表示条件を調べるアクセサ。ClientInstance・ローカルプレイヤー取得経路の解決とネイティブ座標HUDに使用。
///
/// The whole "show coordinates?" accessor, from its first byte:
///
/// ```text
///   0  sub rsp, 28
///   4  mov rax, [rcx + ??]          ; liveness flag
///  29  mov rcx, [rcx + ??]          ; -> ClientInstance        (byte 32)
///  36  mov rax, [rax + ????????]    ; vtable: local player     (bytes 39..43)
///  54  mov rcx, [rax + ????????]    ; player -> level
///  64  mov rax, [rax + ????????]    ; vtable: game rules
/// 108  cmp byte [rcx + ????????], 1 ; rule type is bool
/// 117  mov al, 1
/// 119  cmp byte [rcx + ????????], 0 ; rule value
/// 126  jne +2                       ; <- native HUD patch (75 -> EB)
/// 128  xor eax, eax
/// 130  add rsp, 28 / ret
/// ```
pub(crate) const ACCESSOR_SIGNATURE: &str = "48 83 EC 28 48 8B 41 ?? 48 85 C0 74 ?? 80 38 00 75 ?? C7 04 25 00 00 00 00 DE C0 AD DE 48 8B 49 ?? 48 8B 01 48 8B 80 ?? ?? 00 00 FF 15 ?? ?? ?? ?? 48 85 C0 74 ?? 48 8B 88 ?? ?? 00 00 48 8B 01 48 8B 80 ?? ?? 00 00 FF 15 ?? ?? ?? ?? 48 8B 48 ?? 48 85 C9 74 ?? 48 8B 40 ?? 48 29 C8 48 C1 E8 03 69 C0 ?? ?? ?? ?? 83 F8 ?? 7C ?? 80 B9 ?? ?? 00 00 01 75 ?? B0 01 80 B9 ?? ?? 00 00 00 75 02 31 C0 48 83 C4 28 C3";

/// 座標テキスト生成中の位置・当たり判定の参照。ActorのStateVectorとAABBのオフセットを取得。
///
/// From the game's own coordinate text:
///
/// ```text
///  0  mov rcx, [rax + ????????]   ; player -> position   (bytes 3..7)
///  7  mov rax, [rax + ????????]   ; player -> box        (bytes 10..14)
/// 14  movss xmm0, [rcx]           ; x
/// 18  movss xmm6, [rcx + 8]       ; z
/// 23  movss xmm7, [rax + 4]       ; box min y
/// ```
pub(crate) const ACTOR_SIGNATURE: &str =
    "48 8B 88 ?? ?? 00 00 48 8B 80 ?? ?? 00 00 F3 0F 10 01 F3 0F 10 71 08 F3 0F 10 78 04";

/// ネイティブ座標HUDの表示可否チェック。gameruleを書き換えずにHUDの分岐をパッチする。
/// 一致位置+8のJNE命令のopcodeを85→87（JA）に変更し、boolの0と1を通す。
pub(crate) const VISIBILITY_SIGNATURE: &str =
    "80 BA ?? ?? 00 00 01 0F 85 ?? ?? 00 00 48 8B 86 ?? ?? 00 00 48 8B 48 48";

/// ネイティブ座標HUDのテキスト生成チェック。座標文字列を生成する分岐のパッチに使用。
/// VISIBILITY_SIGNATUREと同様に、一致位置+8のopcodeだけを85→87へ変更する。
pub(crate) const TEXT_SIGNATURE: &str =
    "80 B9 ?? ?? 00 00 01 0F 85 ?? ?? 00 00 8B 05 ?? ?? ?? ?? 89 45 F8 48 8B 05";

// 移動・クリック・マウス入力

/// InventoryMove用の移動許可判定。移動システムの特定呼出元・現在のClientInstanceだけを許可する。
pub(crate) const MOVE_ALLOWED_SIGNATURE: &str =
    "48 8B 49 08 48 8B 01 48 8B 80 E0 04 00 00 48 8B 15 ?? ?? ?? ?? 48 FF E2";
/// InventoryMove用。入力を消去する分岐直前の移動許可呼出箇所。呼出復帰先は一致位置+43。
pub(crate) const MOVE_ALLOWED_CALL_SIGNATURE: &str = "48 8B 0F 48 85 C9 0F 84 ?? ?? ?? ?? 4C 89 44 24 60 48 89 94 24 80 00 00 00 4C 89 4C 24 50 48 8B 01 48 8B 40 08 FF 15 ?? ?? ?? ?? 84 C0 0F 84 ?? ?? ?? ?? 48 89 7C 24 58";

/// InventoryMove用。ClientInstanceから画面スタックを取得する関数。メンバー位置を命令から読む。
pub(crate) const SCREEN_MANAGER_SIGNATURE: &str = "56 57 53 48 83 EC 20 48 8B B9 E8 05 00 00 48 85 FF 0F 84 ?? ?? ?? ?? 0F 57 C0 0F 11 02 48 C7 42 10 00 00 00 00 48 8B 47 10";
/// InventoryMove用。最上位の画面の名前を取得するスタック処理。レイアウトとvtableを照合する。
pub(crate) const TOP_SCREEN_SIGNATURE: &str = "56 48 83 EC 20 48 8B 41 20 48 39 41 18 74 ?? 48 8B 48 E8 48 85 C9 74 ?? 48 8B 01 48 8B 80 B8 01 00 00";
/// InventoryMove用。JSON画面名の取得処理。最上位画面の委譲先を照合し、チャット等と区別する。
pub(crate) const SCREEN_NAME_SIGNATURE: &str = "55 41 57 41 56 41 54 56 57 53 48 81 EC 80 00 00 00 48 8D AC 24 80 00 00 00 48 C7 45 F8 FE FF FF FF 48 89 D6 48 8B 41 50 48 85 C0 0F 84 ?? ?? ?? ?? 48 8B 50 10";

/// ClientInstanceからMoveInputComponentを取得。autosprintと自動ジャンプの通常入力経路に使用。
pub(crate) const MOVE_SIGNATURE: &str = "48 83 EC 28 48 8B 01 48 8B 80 F0 00 00 00 FF 15 ?? ?? ?? ?? 48 85 C0 0F 84 ?? ?? ?? ?? 48 8B 50 10 8B 48 18 8B 42 50 4C 8B 42 48 44 29 C0 C1 E8 03 FF C8 25 87 18 8B 01";

/// 通常の左ボタン押下処理。autoclickerとSDKクリック、コンソール操作時の攻撃抑止に使用。
pub(crate) const ATTACK_DOWN_SIGNATURE: &str = "41 56 56 57 53 48 83 EC 28 4C 89 C6 49 8B 00 48 8B 80 D8 04 00 00 4C 89 C1 FF 15 ?? ?? ?? ?? 84 C0 0F 84 ?? ?? ?? ?? 48 8B 06 48 8B 80 B0 09 00 00 48 89 F1 BA 44 00 00 00 FF 15 ?? ?? ?? ?? 48 8B 06 48 8B 80 08 09 00 00";

/// 通常の左ボタン解放処理。クリックの終了とメニューを開いたときの押下状態解除に使用。
pub(crate) const ATTACK_UP_SIGNATURE: &str = "56 57 48 83 EC 28 4C 89 C6 49 8B 00 48 8B 80 F0 00 00 00 4C 89 C1 FF 15 ?? ?? ?? ?? 48 85 C0 74 31 48 89 C7 48 8D 88 58 07 00 00 E8 ?? ?? ?? ?? 84 C0 75 1E";

/// ゲームの入力更新。移動・クリック・KillAura・SDKの処理をゲーム側の更新スレッドで実行。
pub(crate) const INPUT_TICK_SIGNATURE: &str = "56 57 53 48 83 EC 30 48 89 CE 48 8B 81 78 FF FF FF 48 81 C6 78 FF FF FF 48 8B 80 F0 00 00 00 48 89 F1 FF 15 ?? ?? ?? ?? 48 85 C0 0F 84 ?? ?? ?? ?? 48 8B 48 10 8B 40 18";

/// 通常の移動ボタン押下。bit 13=前進、0=スニーク、8=スプリント、7=ジャンプ。
pub(crate) const TRACK_DOWN_SIGNATURE: &str = "56 57 48 83 EC 28 4C 89 C7 48 89 CE 49 8B 00 48 8B 80 18 05 00 00 4C 89 C1 FF 15 ?? ?? ?? ?? 84 C0 0F 85 FE 00 00 00 48 8B 07 48 8B 80 F0 00 00 00 48 89 F9 FF 15 ?? ?? ?? ?? 48 85 C0 0F 84 E2 00 00 00";

/// 通常の移動ボタン解放。自動で加えた前進・スニーク・スプリント・ジャンプを解除。
pub(crate) const TRACK_UP_SIGNATURE: &str = "56 57 48 83 EC 28 4C 89 C7 48 89 CE 49 8B 00 48 8B 80 18 05 00 00 4C 89 C1 FF 15 ?? ?? ?? ?? 84 C0 0F 85 08 01 00 00 48 8B 07 48 8B 80 F0 00 00 00 48 89 F9 FF 15 ?? ?? ?? ?? 48 85 C0 0F 84 EC 00 00 00";

/// 通常のスプリント切り替え処理の内部アンカー。手動入力の所有状態を追跡する。
/// 一致位置は関数入口ではないため、使用側で0x107を引いて入口を求める。
pub(crate) const SPRINT_TOGGLE_SIGNATURE: &str = "48 6B CA 64 80 74 08 11 01 48 83 C4 20 5E C3";

/// MouseDevice::feedの入口。メニュー表示中の視点移動とホイール入力（button 4）を遮断し、閉じると復帰。
pub(crate) const MOUSE_FEED_SIGNATURE: &str = "41 57 41 56 41 55 41 54 56 57 55 53 48 83 EC 48 44 89 CF 44 89 C3 89 D5 48 89 CE 44 0F B7 A4 24 C0 00 00 00 44 0F B7 AC 24 B8 00 00 00 44 0F B7 BC 24 B0 00 00 00";

// 戦闘・ブロック

/// GameModeの攻撃専用ラッパー。KillAuraで対象へダメージを与える処理（vtable +0x78）。
/// action=1を内部処理に渡す。vtable +0x70の右クリック操作とは別の関数。
pub(crate) const ATTACK_SIGNATURE: &str = "4D 89 C1 41 B0 01 E9 ?? ?? ?? ?? CC CC CC CC CC 55 41 57 41 56 41 54 56 57 53 48 81 EC F0 01 00 00 48 8D AC 24 80 00 00 00 48 C7 85 68 01 00 00 FE FF FF FF 4C 89 CB 45 89 C6 49 89 D7 48 89 CF";

/// ローカルプレイヤーの腕振り処理。KillAuraの攻撃に通常のスイングアニメーションを付ける。
pub(crate) const SWING_SIGNATURE: &str = "56 57 55 53 48 81 EC 88 00 00 00 44 89 C7 89 D3 48 89 CE 80 B9 5D 04 00 00 01 75 ?? 8B AE 30 04 00 00 0F B6 96 5C 04 00 00 48 89 F1 E8 ?? ?? ?? ??";

/// BlockSourceから整数座標のブロックを取得。鉱石・チェストのblockesp走査に使用。
/// RIP相対参照・分岐距離はワイルドカード。ストレージ+0x30を固定し、別レイヤーの+0x38と区別する。
pub(crate) const GET_BLOCK_SIGNATURE: &str = "56 57 48 83 EC 38 48 89 CF 48 8B 05 ?? ?? ?? ?? 48 31 E0 48 89 44 24 30 8B 42 04 0F BF 49 3A 39 C8 0F 8C ?? ?? ?? ?? 0F BF 4F 38 39 C8 0F 8D ?? ?? ?? ?? 48 89 D6 8B 02 C1 F8 04 89 44 24 28 8B 42 08 C1 F8 04 89 44 24 2C 48 8B 07 48 8B 80 48 01 00 00 48 8D 54 24 28 48 89 F9 FF 15 ?? ?? ?? ?? 48 85 C0 74 ?? 44 0F B7 46 04 66 44 2B 47 3A 41 0F BF C8 C1 E9 04 48 0F BF C9 48 8B 90 C8 01 00 00 48 8B 80 D0 01 00 00 48 29 D0 48 C1 F8 03 49 B9 C5 4E EC C4 4E EC C4 4E 4C 0F AF C8 49 39 C9 76 ?? 48 6B C1 68 48 8B 4C 02 30 48 85 C9 74 ?? 0F B7 46 08";

// チャット

/// 画面に表示される受信チャットの入口。SDKへ受信済み本文のコピーを渡す。
pub(crate) const CHAT_RECEIVE_SIGNATURE: &str = "55 41 57 41 56 41 55 41 54 56 57 53 48 81 EC B8 01 00 00 48 8D AC 24 80 00 00 00 48 C7 85 30 01 00 00 FE FF FF FF 44 89 85 EC 00 00 00 48 89 CE 48 81 C1 50 01 00 00";

/// 送信確定したチャットの入口。SDKへ送信済み本文のコピーを渡す。
/// ChatRepeatも、HUD/チャット画面更新の有効なScreenModel参照を借りてこの通常送信経路を使う。
pub(crate) const CHAT_SENT_SIGNATURE: &str = "55 41 57 41 56 41 55 41 54 56 57 53 48 81 EC 18 02 00 00 48 8D AC 24 80 00 00 00 48 C7 85 90 01 00 00 FE FF FF FF 48 89 D7 48 89 CE 48 8B 41 48 48 85 C0";

/// ChatScreenControllerのフレーム更新。チャット表示中のChatRepeatで、
/// callbackの生存中だけ+0xD48のScreenModelを借り、入力欄には触れずに送信する。
pub(crate) const CHAT_TICK_SIGNATURE: &str = "55 41 57 41 56 41 55 41 54 56 57 53 48 81 EC B8 00 00 00 48 8D AC 24 80 00 00 00 0F 29 75 20 48 C7 45 18 FE FF FF FF 48 89 CF E8 ?? ?? ?? ?? 89 45 EC 80 BF A0 0D 00 00 01";

// 視野角・明るさ

/// LevelRendererPlayerの視野角取得関数。zoomで返されるFOVを調整。
///
/// `float LevelRendererPlayer::getFov(this, float partialTicks, bool applyEffects)`,
/// from its first byte:
///
/// ```text
///   0  push rsi / push rdi / push rbx / sub rsp, 60
///   7  movaps [rsp+50], xmm8 / [rsp+40], xmm7 / [rsp+30], xmm6
///  23  mov ebx, r8d                 ; applyEffects
///  26  movaps xmm6, xmm1            ; partialTicks
///  29  mov rsi, rcx                 ; this
///  32  test r8b, r8b / je ...       ; false: the fixed 70 degrees (held item)
///  41  mov rax, [rsi + ????????]
///  48  mov rcx, [rax + ????????]    ; -> options
///  55  mov rax, [rcx] / mov rax, [rax + ????????]
///  65  lea rdx, [rsp+28]
///  70  mov r8d, ????????            ; the FOV option's id
///  76  call [guard dispatch]
/// ```
pub(crate) const FOV_SIGNATURE: &str = "56 57 53 48 83 EC 60 44 0F 29 44 24 50 0F 29 7C 24 40 0F 29 74 24 30 44 89 C3 0F 28 F1 48 89 CE 45 84 C0 0F 84 ?? ?? ?? ?? 48 8B 86 ?? ?? 00 00 48 8B 88 ?? ?? 00 00 48 8B 01 48 8B 80 ?? ?? 00 00 48 8D 54 24 28 41 B8 ?? ?? 00 00 FF 15";

/// gfx_gammaのオプション名を構築する命令。fullbright用のgammaオプションIDを探す手掛かり。
/// 関数入口ではなく、後続の登録コードからIDを読み取る。
pub(crate) const GAMMA_NAME_ANCHOR: &str = "48 B8 67 66 78 5F 67 61 6D 6D";

/// fullbright用のgamma取得関数。登録コードから得たオプションIDを埋め込み、他のfloat設定と区別する。
/// `float Options::getGamma(this)` for a given option id, from its first byte:
///
/// ```text
///   0  sub rsp, 38 / stack cookie
///  19  mov rax, [rcx] / mov rax, [rax + 8]     ; vtable: option by id
///  26  lea rdx, [rsp+28]
///  31  mov r8d, <id>
///  37  call [guard dispatch]
///  43  mov rcx, [rsp+28]                        ; walk to the effective option
///  ..  movss xmm0, [rax + ??]                   ; its value
/// ```
///
/// Every float option has a getter of this shape; only the id tells them apart.
pub(crate) fn gamma_signature(id: u32) -> String {
    with_id(GAMMA_GETTER_SHAPE, GAMMA_ID_AT, id)
}
/// `gamma_signature` with the id left open: the getters of every option of
/// this shape (7 in 1.26.6301.0), found in the shared pass and then told apart
/// by the id at `GAMMA_ID_AT`.
pub(crate) const GAMMA_GETTER_SHAPE: &str = "48 83 EC 38 48 8B 05 ?? ?? ?? ?? 48 31 E0 48 89 44 24 30 48 8B 01 48 8B 40 08 48 8D 54 24 28 41 B8 ?? ?? ?? ?? FF 15 ?? ?? ?? ?? 48 8B 4C 24 28 48 89 C8 48 8B 49 08 48 8B 89 ?? ?? 00 00 48 85 C9 75 ?? F3 0F 10 40 ??";
pub(crate) const GAMMA_ID_AT: usize = 33;

/// `shape` with the little-endian `id` written over the four tokens at `at`.
fn with_id(shape: &str, at: usize, id: u32) -> String {
    let id = id.to_le_bytes().map(|b| format!("{b:02X}"));
    shape
        .split_whitespace()
        .enumerate()
        .map(|(i, token)| match i.checked_sub(at) {
            Some(k) if k < 4 => id[k].as_str(),
            _ => token,
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// ctrl_sensitivity2のオプション名を読み込む登録コード。zoom中の感度調整用に感度オプションIDを探す手掛かり。
/// 同じ形の命令列は他の17文字のオプション名にもあるため、RIP相対の参照先が`SENSITIVITY_NAME`であることも確認する。
///
/// ```text
///  0  movups xmm0, [rip + ????????]   ; the first 16 bytes of the name
///  7  movups [reg], xmm0
/// 10  mov word [reg + 10], 0032        ; its 17th character "2" and the terminator
/// ```
pub(crate) const SENSITIVITY_NAME_ANCHOR: &str = "0F 10 05 ?? ?? ?? ?? 0F 11 ?? 66 C7 ?? 10 32 00";
/// The option name the anchor's RIP-relative operand must point at.
pub(crate) const SENSITIVITY_NAME: &[u8] = b"ctrl_sensitivity2";

/// zoom中のマウス感度調整に使う感度取得関数。登録コードから得たオプションIDを埋め込み、入力モード別の他の設定と区別する。
/// `float Options::getSensitivity(this, uint inputMode)` for a given option id, from its first byte:
///
/// ```text
///   0  push rsi / push rbx / sub rsp, 38
///   6  mov ebx, edx                             ; inputMode
///   8  stack cookie
///  23  mov rax, [rcx] / mov rax, [rax + 8]     ; vtable: option by id
///  30  lea rdx, [rsp+28]
///  35  mov r8d, <id>
///  41  call [guard dispatch]
///  47  mov rcx, [rsp+28]                        ; then: per-input-mode value lookup
/// ```
///
/// Seven per-input-mode options have a getter of this shape; only the id tells them apart.
pub(crate) fn sensitivity_signature(id: u32) -> String {
    with_id(SENSITIVITY_GETTER_SHAPE, SENSITIVITY_ID_AT, id)
}
/// `sensitivity_signature` with the id left open (7 matches in 1.26.6301.0).
pub(crate) const SENSITIVITY_GETTER_SHAPE: &str = "56 53 48 83 EC 38 89 D3 48 8B 05 ?? ?? ?? ?? 48 31 E0 48 89 44 24 30 48 8B 01 48 8B 40 08 48 8D 54 24 28 41 B8 ?? ?? ?? ?? FF 15 ?? ?? ?? ?? 48 8B 4C 24 28";
pub(crate) const SENSITIVITY_ID_AT: usize = 37;

/// MiningHUD: GameMode::stopDestroyBlock。進捗フィールドを読み出して0へ戻す処理。
/// +19=進捗位置、+35=リセット位置。同一であることとGameMode vtable+0x20を照合。呼び出し・フックは行わない。
pub(crate) const MINING_STOP_SIGNATURE: &str = "56 48 83 EC 20 48 89 CE 48 8B 89 ?? ?? 00 00 F3 0F 10 56 ?? 48 8B 01 48 8B 40 40 FF 15 ?? ?? ?? ?? C7 46 ?? 00 00 00 00 48 C7 86 ?? ?? 00 00 00 00 00 00 48 83 C4 20 5E C3";
/// MiningHUD: continueDestroyBlockの実際の進捗加算。+8と+13のフィールド位置が停止処理と一致することを確認。
pub(crate) const MINING_PROGRESS_SIGNATURE: &str =
    "F3 0F 59 FE F3 0F 58 7E ?? F3 0F 11 7E ?? 48 8B 45 18 48 39 86 98 00 00 00 0F 8D ?? ?? ?? ??";
/// MiningHUD: 採掘対象の整数座標3軸とPlayerの採掘中フラグ。座標位置と所有Player(+8)を命令から照合。
pub(crate) const MINING_TARGET_SIGNATURE: &str = "8B 07 3B 46 ?? 0F 85 ?? ?? ?? ?? 8B 47 04 3B 46 ?? 0F 85 ?? ?? ?? ?? 8B 47 08 3B 46 ?? 0F 85 ?? ?? ?? ?? 48 8B 46 08 80 B8 ?? ?? 00 00 01 0F 85 ?? ?? ?? ??";
/// MiningHUD: GameMode::continueDestroyBlockの入口。派生GameModeでも共有するvtable+0x18を照合する。
pub(crate) const MINING_CONTINUE_SIGNATURE: &str = "55 41 57 41 56 41 55 41 54 56 57 53 48 81 EC D8 00 00 00 48 8D AC 24 80 00 00 00 0F 29 7D 40 0F 29 75 30 48 C7 45 28 FE FF FF FF 4D 89 CF 44 88 45 27 48 89 D7 48 89 CE 48 8B 9D C0 00 00 00 48 8B 89 B8 00 00 00 48 8B 01 48 8B 40 10 48 8D 55 18 FF 15 ?? ?? ?? ?? C6 03 00 48 8B 4E 08";

/// KillAura / TriggerBot: 派生GameModeの攻撃専用ラッパー（vtable+0x78）。
/// モード固有の許可判定を維持し、最後は基本GameModeと同じattack helperへ分岐する。右クリック経路ではない。
pub(crate) const DERIVED_ATTACK_SIGNATURE: &str = "4D 89 C1 80 B9 C8 00 00 00 01 75 0C 80 3D ?? ?? ?? ?? 00 74 03 31 C0 C3 41 B0 01 E9 ?? ?? ?? ??";

/// 上記の固定シグネチャすべて。最初の検索時にゲームのコードを1回だけ走査して、まとめて位置を求める
/// （`modules::patch`）。シグネチャを追加したらここにも加える（テストで確認する）。
/// ID入りの`gamma_signature`・`sensitivity_signature`は、IDを伏せた`*_GETTER_SHAPE`の一致からIDで絞り込む。
/// ユーザー指定パッチは対象外で、個別に検索する。
// Build-specific addresses, derived at run time on builds that were not
// analyzed (`layout::derive`). Each RIP-relative operand at the given offset
// refers to the address; found unique in 1.26.6301.0 and 1.26.65.1 and
// checked there against the analyzed tables (`layout` tests).
/// `mov rcx, [rip+air registry]` (operand at +0).
pub(crate) const AIR_REGISTRY_SIGNATURE: &str = "48 8B 0D ?? ?? ?? ?? 48 85 C9 75 0B C7 04 25 00 00 00 00 DE C0 AD DE 48 8B 09 48 8B 89 40 02 00 00 48 89 08 48 C7 40 08";
/// `lea rax, [rip+BlockSource vtable]; mov [rcx], rax` (operand at +0).
pub(crate) const BLOCK_SOURCE_VTABLE_SIGNATURE: &str =
    "48 8D 05 ?? ?? ?? ?? 48 89 01 E8 ?? ?? ?? ?? 41 89 46 18 45 88 6E 1C 45";
/// `lea rax, [rip+request manager vtable]` in its constructor (operand at +8).
pub(crate) const REQUEST_MANAGER_VTABLE_SIGNATURE: &str = "83 EC 20 89 D7 48 89 CE 48 8D 05 ?? ?? ?? ?? 48 89 01 48 8B 89 80 00 00 00 48 85 C9 74 13 F0 FF";
/// `lea r15, [rip+ItemStack vtable]` (operand at +0).
pub(crate) const ITEM_STACK_VTABLE_SIGNATURE: &str =
    "4C 8D 3D ?? ?? ?? ?? 4C 89 BD B0 01 00 00 48 8D 85 30 02 00 00 C7 85 30";
/// The two request scope vtables, each `lea rax, [rip+vtable]` (operand at +0).
pub(crate) const REQUEST_SCOPE_VTABLE_SIGNATURE: &str =
    "48 8D 05 ?? ?? ?? ?? 48 89 06 E9 ?? ?? ?? ?? 49 8B 40 08 48 C7 00 00 00";
/// 耐久保護: 2番目の要求スコープvtable。1番目とは異なるテーブルを指すことを照合する。
pub(crate) const REQUEST_SCOPE_VTABLE_2_SIGNATURE: &str =
    "48 8D 05 ?? ?? ?? ?? 48 89 06 48 89 7E 08 48 89 5E 10 48 89 76 38 C6 46";
/// Signatures used only to derive addresses: a build where they are missing
/// is still recognized as a patch of an analyzed build (read-only features).
pub(crate) const DERIVATION: &[&str] = &[
    AIR_REGISTRY_SIGNATURE,
    BLOCK_SOURCE_VTABLE_SIGNATURE,
    REQUEST_MANAGER_VTABLE_SIGNATURE,
    ITEM_STACK_VTABLE_SIGNATURE,
    REQUEST_SCOPE_VTABLE_SIGNATURE,
    REQUEST_SCOPE_VTABLE_2_SIGNATURE,
];

/// Player List: Level roster getter slot, MSVC list head/link walk and name field.
pub(crate) const PLAYER_ROSTER_WALK_SIGNATURE: &str = "48 8B 80 F8 09 00 00 4C 89 F1 FF 15 ?? ?? ?? ?? 90 48 8B 40 08 48 89 C7 66 2E 0F 1F 84 00 00 00 00 00 48 8B 3F 48 39 C7 0F 84 ?? ?? ?? ?? 80 BF E1 00 00 00 01 75 EB 0F 57 C0 0F 29 85 ?? ?? ?? ?? 0F 29 85 ?? ?? ?? ?? 4C 8B 7F 48 48 83 7F 50 10 72 06 4C 8B 67 38 EB 04 4C 8D 67 38";

/// Optional read-only additions validate their own layouts (or require an
/// exact build). Their absence must not revoke the older core's patch checks.
/// This list does not grant permission to run any feature.
pub(crate) const OPTIONAL_READ_SIGNATURES: &[&str] = &[
    PLAYER_ROSTER_WALK_SIGNATURE,
    SKIN_ID_FIELD_SIGNATURE,
    ACTOR_SKIN_REF_SIGNATURE,
    CLIENT_SKIN_SIGNATURE,
    SKIN_IMAGE_FIELD_SIGNATURE,
    SKIN_IMAGE_COPY_SIGNATURE,
    SKIN_GEOMETRY_FIELD_SIGNATURE,
    SKIN_ANIMATION_FIELD_SIGNATURE,
    SOUND_INIT_SIGNATURE,
    SOUND_DEBUG_SIGNATURE,
    SOUND_ENGINE_STATS_SIGNATURE,
    LEVEL_ID_FIELD_SIGNATURE,
    LEVEL_DATA_SIGNATURE,
    LEVEL_NAME_FIELD_SIGNATURE,
    LEVEL_DIFFICULTY_SIGNATURE,
    BIOME_NAME_FIELD_SIGNATURE,
    GET_BIOME_SIGNATURE,
    GET_EXTRA_BLOCK_SIGNATURE,
];
pub(crate) fn baseline_required(pattern: &str) -> bool {
    // Freecam separately requires full native compatibility and its own
    // camera layout check; adding it must not narrow older feature support.
    !DERIVATION.contains(&pattern)
        && !OPTIONAL_READ_SIGNATURES.contains(&pattern)
        && pattern != CAMERA_POSITION_COPY_SIGNATURE
        && pattern != RENDER_CAMERA_ORIGIN_SIGNATURE
}

#[derive(Clone, Copy)]
pub(crate) struct Entry {
    pub name: &'static str,
    pub pattern: &'static str,
}
macro_rules! catalog {
    ($($name:ident),* $(,)?) => {
        pub(crate) const BUILT_IN: &[&str] = &[$($name),*];
        pub(crate) const CATALOG: &[Entry] = &[$(Entry { name: stringify!($name), pattern: $name }),*];
    };
}
catalog! {
    PLAYER_ROSTER_WALK_SIGNATURE,
    SKIN_ID_FIELD_SIGNATURE,
    SKIN_GEOMETRY_FIELD_SIGNATURE,
    SKIN_ANIMATION_FIELD_SIGNATURE,
    ACTOR_SKIN_REF_SIGNATURE,
    CLIENT_SKIN_SIGNATURE,
    SKIN_IMAGE_FIELD_SIGNATURE,
    SKIN_IMAGE_COPY_SIGNATURE,
    SOUND_INIT_SIGNATURE,
    LEVEL_ID_FIELD_SIGNATURE,
    BIOME_NAME_FIELD_SIGNATURE,
    GET_BIOME_SIGNATURE,
    GET_EXTRA_BLOCK_SIGNATURE,
    SOUND_DEBUG_SIGNATURE,
    SOUND_ENGINE_STATS_SIGNATURE,
    LEVEL_DIFFICULTY_SIGNATURE,
    LEVEL_DATA_SIGNATURE,
    LEVEL_NAME_FIELD_SIGNATURE,
    AIR_REGISTRY_SIGNATURE,
    BLOCK_SOURCE_VTABLE_SIGNATURE,
    REQUEST_MANAGER_VTABLE_SIGNATURE,
    ITEM_STACK_VTABLE_SIGNATURE,
    REQUEST_SCOPE_VTABLE_SIGNATURE,
    REQUEST_SCOPE_VTABLE_2_SIGNATURE,
    LEVEL_TIME_SIGNATURE,
    CLOCK_HANDLE_SIGNATURE,
    WEATHER_FIELDS_SIGNATURE,
    BLOCK_LIGHT_SIGNATURE,
    CAMERA_PLAYER_SYNC_SIGNATURE,
    CAMERA_POSITION_COPY_SIGNATURE,
    RENDER_CAMERA_ORIGIN_SIGNATURE,
    FISH_EVENT_SIGNATURE,
    INVENTORY_REQUEST_BEGIN_SIGNATURE,
    INVENTORY_STACK_COPY_SIGNATURE,
    INVENTORY_SET_SIGNATURE,
    ITEM_DAMAGE_SIGNATURE,
    ITEM_MAX_DAMAGE_SIGNATURE,
    COMPONENT_MAX_DAMAGE_SIGNATURE,
    INVENTORY_ITEM_SIGNATURE,
    ARMOR_ITEM_SIGNATURE,
    SELECT_SLOT_SIGNATURE,
    PROXY_SELECT_SIGNATURE,
    USE_DOWN_SIGNATURE,
    USE_UP_SIGNATURE,
    USE_ALLOWED_SIGNATURE,
    EFFECT_LAYOUT_SIGNATURE,
    METADATA_ACCESSORS_SIGNATURE,
    ACCESSOR_SIGNATURE,
    ACTOR_SIGNATURE,
    VISIBILITY_SIGNATURE,
    TEXT_SIGNATURE,
    MOVE_ALLOWED_SIGNATURE,
    MOVE_ALLOWED_CALL_SIGNATURE,
    SCREEN_MANAGER_SIGNATURE,
    TOP_SCREEN_SIGNATURE,
    SCREEN_NAME_SIGNATURE,
    MOVE_SIGNATURE,
    ATTACK_DOWN_SIGNATURE,
    ATTACK_UP_SIGNATURE,
    INPUT_TICK_SIGNATURE,
    TRACK_DOWN_SIGNATURE,
    TRACK_UP_SIGNATURE,
    SPRINT_TOGGLE_SIGNATURE,
    MOUSE_FEED_SIGNATURE,
    ATTACK_SIGNATURE,
    SWING_SIGNATURE,
    GET_BLOCK_SIGNATURE,
    CHAT_RECEIVE_SIGNATURE,
    CHAT_SENT_SIGNATURE,
    CHAT_TICK_SIGNATURE,
    FOV_SIGNATURE,
    GAMMA_NAME_ANCHOR,
    GAMMA_GETTER_SHAPE,
    SENSITIVITY_NAME_ANCHOR,
    SENSITIVITY_GETTER_SHAPE,
    MINING_STOP_SIGNATURE,
    MINING_PROGRESS_SIGNATURE,
    MINING_TARGET_SIGNATURE,
    MINING_CONTINUE_SIGNATURE,
    DERIVED_ATTACK_SIGNATURE,
}

#[cfg(test)]
mod tests {
    /// Every `const …: &str` pattern in this file, as written in the source.
    fn declared() -> Vec<(String, String)> {
        let source = include_str!("signatures.rs");
        let source = &source[..source.find("#[cfg(test)]").unwrap_or(source.len())];
        let mut out = Vec::new();
        for (at, _) in source.match_indices("pub(crate) const ") {
            let rest = &source[at + "pub(crate) const ".len()..];
            let name = rest[..rest.find(':').unwrap()].trim().to_string();
            let declaration = &rest[..rest.find(';').unwrap()];
            if !declaration.contains(": &str") {
                continue;
            }
            let open = declaration.find('"').unwrap();
            let close = declaration.rfind('"').unwrap();
            out.push((name, declaration[open + 1..close].to_string()));
        }
        out
    }

    #[test]
    fn every_fixed_signature_is_resolved_in_the_shared_pass_and_parses() {
        let declared = declared();
        assert!(declared.len() >= 40, "{declared:?}");
        for (name, text) in &declared {
            assert!(
                super::BUILT_IN.contains(&text.as_str()),
                "{name} is missing from BUILT_IN"
            );
        }
        assert_eq!(super::BUILT_IN.len(), declared.len());
        assert!(super::BUILT_IN.contains(&super::GAMMA_GETTER_SHAPE));
        assert!(super::BUILT_IN.contains(&super::SENSITIVITY_GETTER_SHAPE));
        for text in super::BUILT_IN {
            assert!(text.parse::<sigscan::Pattern>().is_ok(), "{text}");
        }
        assert_eq!(super::CATALOG.len(), super::BUILT_IN.len());
        for (i, entry) in super::CATALOG.iter().enumerate() {
            assert!(declared
                .iter()
                .any(|(name, pattern)| name == entry.name && pattern == entry.pattern));
            assert!(!super::CATALOG[..i].iter().any(|e| e.name == entry.name));
        }
    }
    #[test]
    fn optional_read_failures_do_not_revoke_the_core_baseline() {
        for signature in super::OPTIONAL_READ_SIGNATURES {
            assert!(super::BUILT_IN.contains(signature));
            assert!(!super::baseline_required(signature));
        }
        assert!(super::baseline_required(super::ACCESSOR_SIGNATURE));
        assert!(super::baseline_required(super::ACTOR_SIGNATURE));
        assert!(super::baseline_required(super::INPUT_TICK_SIGNATURE));
        assert!(super::baseline_required(super::GET_BLOCK_SIGNATURE));
        assert!(super::baseline_required(super::FISH_EVENT_SIGNATURE));
        assert!(!super::baseline_required(
            super::CAMERA_POSITION_COPY_SIGNATURE
        ));
        assert!(!super::baseline_required(
            super::RENDER_CAMERA_ORIGIN_SIGNATURE
        ));
    }

    #[test]
    fn id_signatures_are_their_shape_with_the_id_filled_in() {
        // The exact text these functions produced before they were derived
        // from the shapes.
        assert_eq!(
            super::gamma_signature(0x32),
            "48 83 EC 38 48 8B 05 ?? ?? ?? ?? 48 31 E0 48 89 44 24 30 48 8B 01 48 8B 40 08 \
             48 8D 54 24 28 41 B8 32 00 00 00 FF 15 ?? ?? ?? ?? 48 8B 4C 24 28 48 89 C8 48 8B 49 08 \
             48 8B 89 ?? ?? 00 00 48 85 C9 75 ?? F3 0F 10 40 ??"
        );
        assert_eq!(
            super::sensitivity_signature(0x1234_5678),
            "56 53 48 83 EC 38 89 D3 48 8B 05 ?? ?? ?? ?? 48 31 E0 48 89 44 24 30 48 8B 01 \
             48 8B 40 08 48 8D 54 24 28 41 B8 78 56 34 12 FF 15 ?? ?? ?? ?? 48 8B 4C 24 28"
        );
        for (shape, at) in [
            (super::GAMMA_GETTER_SHAPE, super::GAMMA_ID_AT),
            (super::SENSITIVITY_GETTER_SHAPE, super::SENSITIVITY_ID_AT),
        ] {
            let tokens: Vec<&str> = shape.split_whitespace().collect();
            // mov r8d, imm32: the id is the open immediate right after `41 B8`.
            assert_eq!(
                &tokens[at - 2..at + 4],
                ["41", "B8", "??", "??", "??", "??"]
            );
        }
    }
}
