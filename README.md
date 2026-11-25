# Mio Client

![Mio Client](assets/mio-mark.svg)

Mio Clientは、Windows x64版のMinecraft Bedrockを対象としたRust製クライアントです。クライアント本体と関連ツールを1つのCargoワークスペースで管理しています。

## プロジェクト構成

crateはすべて [crates/](crates/) にあります。共通の依存とバージョンはルートの `Cargo.toml`（`[workspace.dependencies]`）でまとめて管理しています。

- [client](crates/client/) — クライアント本体
  - `game/` — ゲームのデータを読む層（`native` メモリ・レイアウト・シグネチャ / `level` ワールド・ブロック / `actor` エンティティ / `player` ローカルプレイヤー / `attack` 攻撃 / `render` カメラ）
  - `modules/` — 機能。ClickGUIのカテゴリ別（`movement` / `combat` / `world` / `visual` / `hud` / `other`）
  - `app/` — 設定・プロファイル・キーバインド・コンソール・通知
  - `ui/` — オーバーレイ（メニュー・HUD編集・テーマ）
  - `platform/` — Windows・プロセス連携（フック・入力・クリップボード・チャット）
  - `api/` — SDKのホスト
- [injector](crates/injector/) — Mio Launcher
- [sdk](crates/sdk/) — クライアントと連携するためのRust SDK
- [signature_checker](crates/signature_checker/) — ゲーム更新時の互換性を確認するツール
- [sigscan](crates/sigscan/) — バイトパターン検索ライブラリ
- [shared](crates/shared/) — clientとツールが共有するソース（ブランド・互換性チェック）
- [assets](assets/) — アイコンなどの画像
