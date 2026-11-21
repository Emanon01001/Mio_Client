# Mio Client

![Mio Client](assets/mio-mark.svg)

Mio Clientは、Windows x64版のMinecraft Bedrockを対象としたRust製クライアントです。クライアント本体と関連ツールを1つのCargoワークスペースで管理しています。

## プロジェクト構成

- [client](client/) — クライアント本体
- [injector](injector/) — Mio Launcher
- [sdk](sdk/) — クライアントと連携するためのRust SDK
- [signature_checker](signature_checker/) — ゲーム更新時の互換性を確認するツール
- [sigscan](sigscan/) — バイトパターン検索ライブラリ
