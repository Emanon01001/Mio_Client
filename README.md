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
- [shared](crates/shared/) — clientとツールが共有するソース（ブランド・互換性チェック・リソース埋め込み）
- [assets](assets/) — アイコンなどの画像

## Windows向けの配布

ダウンロードしたファイルを初めて開くと、SmartScreenの警告が表示される場合があります。SmartScreenはファイルと発行元の評価を確認します。コード署名が有効でも、新しいリリースで警告が表示される場合があります。[MicrosoftのSmartScreen解説](https://learn.microsoft.com/windows/apps/package-and-deploy/smartscreen-reputation)

### ビルド時に自動で埋め込むもの

各crateの`build.rs`が、エクスプローラーなどに表示する製品名とバージョン、実行ファイルのアイコンとマニフェストを埋め込みます。[リソース埋め込みコード](crates/shared/winres.rs)

- **バージョン情報**（製品名・説明・バージョン）: injectorの各exe・`Mio_Client.dll`・`mio_signature_checker(.exe/_cli.exe)`すべて。
- **アイコン**: 各exe。
- **マニフェスト**（exeのみ）: `asInvoker`（不要なUAC昇格要求なし）・per-monitor-v2 DPI対応・長パス対応。

PowerShellで表示情報を確認できます。

```powershell
(Get-Item target\release\mio_launcher.exe).VersionInfo | Format-List FileDescription, ProductName, FileVersion
```

### コード署名

信頼されたコード署名証明書で署名すると、Windowsで発行元を確認できます。次の例は、署名用証明書をインストール済みの環境で、ビルドしたEXEとDLLにタイムスタンプ付きの署名を付けます。

```powershell
signtool sign /n "発行者名" /fd SHA256 /tr http://timestamp.digicert.com /td SHA256 `
  target\release\mio_launcher.exe target\release\mio_injector.exe `
  target\release\mio_injector_gui.exe target\release\mio_signature_checker.exe `
  target\release\mio_signature_checker_cli.exe target\release\Mio_Client.dll
```

Microsoft Defenderで誤検知が疑われる場合は、検知名と対象ファイルをMicrosoftへ提出できます。[誤検知の報告窓口](https://www.microsoft.com/wdsi/filesubmission)
