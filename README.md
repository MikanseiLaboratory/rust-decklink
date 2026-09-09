# rust-decklink

Blackmagic DeckLink の映像・音声入出力を、Windows / macOS / Linux から同じ Rust API で扱うクレートです。公開する非同期面は特定ランタイムに依存せず、`Stream` と `Sink` だけを使います。

SDK そのものはこのリポジトリに含めていません。実機を使うときは Blackmagic の Desktop Video ドライバと [DeckLink SDK 16.0](https://www.blackmagicdesign.com/desktopvideo_sdk) を別途用意してください。ライセンス条件は [NOTICE](NOTICE) に書いてあります。

## できること

- デバイス列挙と表示モードの取得
- 映像・音声の capture（形式変更通知つき）
- scheduled playback（preroll、完了結果、停止待ち）
- bounded queue による overflow 制御
- SDK なしで回る mock バックエンド

初期リリースでは ancillary / HDR / profile / DeckControl / IP / encoder / GPU allocator は扱いません。

## 使い方

```rust
use decklink::{DeckLinkContext, PixelFormat};
use futures_util::StreamExt;

let context = DeckLinkContext::connect(); // 実機がなければ mock
let device = context.first_device()?;
let mode = device.display_modes()?.into_iter().next().unwrap();

let mut capture = device
    .capture()
    .video(mode, PixelFormat::YUV_8BIT)
    .queue_capacity(4)
    .start()
    .await?;

while let Some(event) = capture.next().await {
    let _ = event?;
}

capture.shutdown().await?;
```

確実に止めたいときは `shutdown().await` を呼んでください。`Drop` は停止要求だけ送り、完了は待ちません。

## ビルド

```bash
cargo test --workspace
```

実機用 shim を入れる場合:

```bash
set DECKLINK_SDK_DIR=C:\path\to\Blackmagic DeckLink SDK 16.0
cargo test --workspace --features hardware
```

Windows の hardware ビルドには Visual Studio の C++ ツールと `midl.exe` が必要です。Linux は実行時に `libDeckLinkAPI.so` を、macOS は `DeckLinkAPI.framework` を読みます。SDK が見つからないときは stub になり、`DeckLinkContext::new()` は失敗します。mock と `connect()` はそのまま使えます。

## 安全性の要点

- COM オブジェクトは actor スレッドの外へ出しません
- callback 内では `AddRef` と queue 投入だけを行い、ブロックしません
- capture frame の画素は `map_read()` の guard の間だけ借りられます
- output frame は完了 callback までクレート側で保持します
- 未知の HRESULT は丸めず、そのまま保持します

## ライセンス

MIT または Apache-2.0 です。DeckLink SDK とドライバは Blackmagic Design の EULA が別途かかります。
