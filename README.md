# rust-decklink

Blackmagic DeckLink の映像・音声入出力を、Windows / macOS / Linux から同じ Rust API で扱うクレートです。公開する非同期面は特定ランタイムに依存せず、`Stream` と `Sink` だけを使います。

SDK そのものはこのリポジトリに含めていません。実機を使うときは Blackmagic の Desktop Video ドライバと [DeckLink SDK 16.0](https://www.blackmagicdesign.com/desktopvideo_sdk) を別途用意してください。ライセンス条件は [NOTICE](NOTICE) に書いてあります。

## できること

- デバイス列挙と表示モードの取得
- 映像・音声の capture（形式変更通知つき）
- scheduled playback（preroll、完了結果、停止待ち）
- bounded queue による overflow 制御
- SDK なしで回る mock バックエンド

初期リリースでは ancillary / HDR / profile / DeckControl / IP / encoder は扱いません。

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

## 実機での確認

カードが刺さっている PC で、Desktop Video ドライバと SDK 16.0 を入れてから次を実行します。`--hardware` を付けると mock に落ちません。失敗したらドライバか `DECKLINK_SDK_DIR` を先に疑ってください。

```bash
set DECKLINK_SDK_DIR=C:\path\to\Blackmagic DeckLink SDK 16.0

cargo run -p decklink --features hardware --example list_devices -- --hardware
cargo run -p decklink --features hardware --example capture_stats -- --hardware --seconds 10
cargo run -p decklink --features hardware --example capture_frame -- --hardware --out frame.uyvy
cargo run -p decklink --features hardware --example scheduled_playout -- --hardware --seconds 10
cargo run -p decklink --features hardware --example colorbars -- --hardware --seconds 10
cargo run -p decklink --features hardware,wgpu --example wgpu_colorbars -- --hardware --seconds 10
```

`colorbars` と `wgpu_colorbars` は SMPTE RP 219 HD カラーバー、1 kHz（-20 dBFS）ステレオ、水平スクロールを出します。

共通オプションは `--device 0`、`--mode 1080p30`、`--frames 150`、`--audio` です。環境変数 `DECKLINK_DEVICE` / `DECKLINK_MODE` / `DECKLINK_SECONDS` / `DECKLINK_REQUIRE_HARDWARE` でも同じ指定ができます。

`capture_stats` の `no_source` がフレーム数と同じなら、ケーブルか信号源を見てください。`capture_frame` で書いた UYVY は次で再生できます。

```bash
ffplay -f rawvideo -pixel_format uyvy422 -video_size 1920x1080 frame.uyvy
```

## GPU バッファ（eiviz / wgpu）

画素の読み書きは CPU/GPU を意識せず `map_read()` / `AllocatedGpuBuffer::as_mut_slice()` で足ります。DeckLink がロックするのは常に CPU ポインタです。Windows は `VirtualAlloc`、Unix は `posix_memalign(4096)`、macOS Metal は共有ヒープです。GPU BAR / `GPU_UPLOAD` は渡しません。UYVY は width/2 の `Rgba8Unorm` として扱います。

```rust
use decklink::{CaptureEvent, CpuSharedFactory, WgpuSharedFactory};
use std::sync::Arc;

// eiviz の wgpu::Device を渡す（Dx12 / Metal / Vulkan）
let factory = WgpuSharedFactory::new(device.clone(), backend);
let mut capture = device
    .capture()
    .gpu_buffers(Arc::new(factory))
    .start()
    .await?;

while let Some(Ok(CaptureEvent::Sample(sample))) = capture.next().await {
    if let Some(frame) = &sample.video {
        let _pixels = frame.map_read()?.as_bytes();
        if let Some(gpu) = frame.gpu() {
            // 任意: Metal では gpu.wgpu_buffer が同じメモリ。Windows では cpu_ptr のみ。
            let _ = gpu.packed_uyvy_extent();
        }
    }
}
```

`WgpuSharedFactory` は `--features wgpu` です。`backend()` は DeckLink が見るメモリ種別（Windows / Linux は `Cpu`、macOS Metal は `Metal`）です。wgpu の adapter は `wgpu_backend()` です。自前確保なら `GpuBufferFactory` を実装してください。

再生は `ScheduledVideoFrame::from_bytes` か `from_buffer` を使います。完了 callback までバッファを保持してください。GPU コマンドで書いた場合は、schedule の前に fence を待ってください。

## 安全性の要点

- COM オブジェクトは actor スレッドの外へ出しません
- callback 内では `AddRef` と queue 投入だけを行い、ブロックしません
- capture frame の画素は `map_read()` の guard の間だけ借りられます
- output frame は完了 callback までクレート側で保持します
- 未知の HRESULT は丸めず、そのまま保持します

## ライセンス

MIT または Apache-2.0 です。DeckLink SDK とドライバは Blackmagic Design の EULA が別途かかります。
