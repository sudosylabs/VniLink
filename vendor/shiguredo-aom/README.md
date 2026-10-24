# aom-rs

[![crates.io](https://img.shields.io/crates/v/shiguredo_aom.svg)](https://crates.io/crates/shiguredo_aom)
[![docs.rs](https://docs.rs/shiguredo_aom/badge.svg)](https://docs.rs/shiguredo_aom)
[![License](https://img.shields.io/badge/License-Apache%202.0-blue.svg)](https://opensource.org/licenses/Apache-2.0)
[![GitHub Actions](https://github.com/shiguredo/aom-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/shiguredo/aom-rs/actions/workflows/ci.yml)
[![Discord](https://img.shields.io/badge/Discord-%235865F2.svg?logo=discord&logoColor=white)](https://discord.gg/shiguredo)

## About Shiguredo's open source software

We will not respond to PRs or issues that have not been discussed on Discord. Also, Discord is only available in Japanese.

Please read <https://github.com/shiguredo/oss> before use.

## 時雨堂のオープンソースソフトウェアについて

利用前に <https://github.com/shiguredo/oss> をお読みください。

## 概要

[libaom](https://aomedia.googlesource.com/aom) を利用した AV1 エンコーダーおよびデコーダーの Rust バインディングです。

## 特徴

- コーデック対応情報の照会 (`supported_codecs()`)
- AV1 エンコーダー / デコーダー
- 複数の画像フォーマット対応 (I420, YV12, NV12, I422, I444, I42016, I42216, I44416)
- libaom の `aom_codec_enc_cfg_t` フィールドに準拠したエンコーダー設定
- libaom のエンコーダー制御パラメータ (`aom_codec_control`) に準拠した詳細設定
- 複数のエンコーダーモード (GoodQuality, Realtime, AllIntra)
- レート制御 (VBR, CBR, CQ, Q)
- ロスレスエンコード
- カラー情報 (Color Primaries, Transfer Characteristics, Matrix Coefficients, Color Range)
- AV1 インループフィルター制御 (CDEF, リストレーション, ループフィルタ)
- タイル分割 / 行マルチスレッド
- スーパーレゾリューション / 動的リサイズ
- S-Frame (スイッチフレーム) サポート
- デノイズ / フィルムグレイン
- per-frame のキーフレーム強制
- シンボル書き換えによる他ライブラリとの衝突回避 (`shiguredo_aom_` プレフィックス付与)
- prebuilt バイナリによる高速ビルド (デフォルト)
- ソースからのビルドも可能 (`--features source-build`)

## 動作要件

- Ubuntu 24.04 x86_64
- Ubuntu 24.04 arm64
- Ubuntu 22.04 x86_64
- Ubuntu 22.04 arm64
- macOS 26 arm64
- macOS 15 arm64
- Windows 11 x86_64
- Windows Server 2025 x86_64

### ソースビルド時の追加要件

- Git
- C コンパイラ (`build-essential` 等)
- NASM (libaom のアセンブリ最適化に必要)

```bash
# Ubuntu
sudo apt-get install -y build-essential nasm

# macOS
brew install nasm

# Windows
choco install nasm
```

## ビルド

デフォルトでは GitHub Releases から prebuilt バイナリをダウンロードしてビルドします。

```bash
cargo build
```

### ソースからビルド

libaom をソースからビルドする場合は `source-build` feature を有効にしてください。

```bash
cargo build --features source-build
```

### docs.rs 向けビルド

libaom がない環境では、docs.rs 向けのドキュメント生成のみ可能です。

```bash
DOCS_RS=1 cargo doc --no-deps
```

## 使い方

### コーデック対応情報の照会

```rust
use shiguredo_aom::supported_codecs;

let info = supported_codecs();

// デコード対応状況
println!("decoding supported: {}", info.decoding.supported);
println!("decoding hardware_accelerated: {}", info.decoding.hardware_accelerated);

// エンコード対応状況
println!("encoding supported: {}", info.encoding.supported);
println!("encoding hardware_accelerated: {}", info.encoding.hardware_accelerated);

// 対応プロファイル一覧
match &info.encoding.profiles {
    shiguredo_aom::EncodingProfiles::Av1(profiles) => {
        for profile in profiles {
            println!("profile: {:?}", profile);
        }
    }
    shiguredo_aom::EncodingProfiles::Unsupported => {}
}
```

### エンコード

入力は `ImageData` 列挙型で画像フォーマットと各プレーンのデータを渡します。

```rust
use shiguredo_aom::{
    AomRational, EncodeOptions, Encoder, EncoderConfig,
    ImageData, ImageFormat, RateControlMode, Usage,
};

// 必須パラメータを指定して設定を生成
let mut config = EncoderConfig::new(
    1920,              // g_w
    1080,              // g_h
    ImageFormat::I420, // image_format
);

// 必要に応じてオプションパラメータを変更
config.g_usage = Usage::Realtime;
config.rc_end_usage = RateControlMode::Cbr;
config.rc_target_bitrate = 4000; // kbps
config.cpu_used = Some(8);
config.g_threads = Some(4);
config.g_timebase = AomRational { num: 1, den: 30 };

// エンコーダーを作成
let mut encoder = Encoder::new(config)?;

// I420 形式の YUV データをエンコード
let image = ImageData::I420 { y: &y_data, u: &u_data, v: &v_data };
encoder.encode(&image, &EncodeOptions { force_keyframe: false })?;

// キーフレームを強制する場合
encoder.encode(&image, &EncodeOptions { force_keyframe: true })?;

// エンコード済みフレームを取得
while let Some(frame) = encoder.next_frame() {
    let data = frame.data();
    let is_key = frame.is_keyframe();
    println!("encoded: {} bytes, keyframe: {}", data.len(), is_key);
}

// 残りのフレームをフラッシュ
encoder.finish()?;
while let Some(frame) = encoder.next_frame() {
    // ...
}
```

### デコード

```rust
use shiguredo_aom::{Decoder, DecoderConfig};

// デコーダーを作成
let mut decoder = Decoder::new(DecoderConfig::default())?;

// 圧縮データをデコード
decoder.decode(&compressed_data)?;

// デコード済みフレームを取得
while let Some(frame) = decoder.next_frame() {
    let y = frame.y_plane();
    let u = frame.u_plane();
    let v = frame.v_plane();
    let is_high_depth = frame.is_high_depth();
    println!("{}x{} high_depth={}", frame.width(), frame.height(), is_high_depth);
}

// 残りのフレームをフラッシュ
decoder.finish()?;
while let Some(frame) = decoder.next_frame() {
    // ...
}
```

## 設定

### `CodecInfo`

| フィールド | 型 | 説明 |
|---|---|---|
| `codec` | `VideoCodecType` | コーデック種別 |
| `decoding` | `DecodingInfo` | デコード情報 |
| `encoding` | `EncodingInfo` | エンコード情報 |

### `VideoCodecType`

| バリアント | 説明 |
|---|---|
| `Av1` | AV1 |

### `DecodingInfo`

| フィールド | 型 | 説明 |
|---|---|---|
| `supported` | `bool` | デコードに対応しているか |
| `hardware_accelerated` | `bool` | ハードウェアアクセラレーションに対応しているか |

### `EncodingInfo`

| フィールド | 型 | 説明 |
|---|---|---|
| `supported` | `bool` | エンコードに対応しているか |
| `hardware_accelerated` | `bool` | ハードウェアアクセラレーションに対応しているか |
| `profiles` | `EncodingProfiles` | コーデック固有のプロファイル情報 |

### `EncodingProfiles`

| バリアント | 説明 |
|---|---|
| `Av1(Vec<Av1EncodingProfile>)` | AV1 プロファイル一覧 |
| `Unsupported` | プロファイル情報なし |

### `Av1EncodingProfile`

| バリアント | 説明 |
|---|---|
| `Profile0` | 8/10-bit 4:2:0 |
| `Profile1` | 8/10-bit 4:4:4 |
| `Profile2` | 8/10/12-bit 4:2:0, 4:2:2, 4:4:4 |

### `EncoderConfig`

フィールド名は libaom の `aom_codec_enc_cfg_t` および `aom_codec_control` の制御パラメータに準拠しています。

#### 基本設定

| フィールド | 型 | 説明 |
|---|---|---|
| `image_format` | `ImageFormat` | 入力画像フォーマット |
| `g_usage` | `Usage` | エンコーダーの利用モード |
| `g_threads` | `Option<u32>` | スレッド数 (0 は 1 と同等) |
| `g_profile` | `u32` | ビットストリームプロファイル (0, 1, 2) |
| `g_w` | `u32` | フレームの幅 |
| `g_h` | `u32` | フレームの高さ |
| `g_limit` | `Option<u32>` | エンコードする最大フレーム数 |
| `g_forced_max_frame_width` | `Option<u32>` | 強制最大フレーム幅 |
| `g_forced_max_frame_height` | `Option<u32>` | 強制最大フレーム高さ |
| `g_bit_depth` | `Option<u32>` | コーデックのビット深度 (8, 10, 12) |
| `g_input_bit_depth` | `Option<u32>` | 入力フレームのビット深度 |
| `g_timebase` | `AomRational` | タイムベース (例: 30fps なら num=1, den=30) |
| `g_error_resilient` | `bool` | エラー耐性モード |
| `g_pass` | `Option<EncodingPass>` | マルチパスエンコーディングモード |
| `g_lag_in_frames` | `Option<u32>` | 先読みフレーム数 |

#### レート制御

| フィールド | 型 | 説明 |
|---|---|---|
| `rc_end_usage` | `RateControlMode` | レート制御モード |
| `rc_target_bitrate` | `u32` | ターゲットビットレート (kbps) |
| `rc_min_quantizer` | `u32` | 最小量子化値 |
| `rc_max_quantizer` | `u32` | 最大量子化値 |
| `rc_undershoot_pct` | `Option<u32>` | VBR アンダーシュート許容率 (0-100) |
| `rc_overshoot_pct` | `Option<u32>` | VBR オーバーシュート許容率 (0-100) |
| `rc_buf_sz` | `Option<u32>` | デコーダーバッファサイズ (ms) |
| `rc_buf_initial_sz` | `Option<u32>` | デコーダーバッファ初期サイズ (ms) |
| `rc_buf_optimal_sz` | `Option<u32>` | デコーダーバッファ最適サイズ (ms) |
| `rc_dropframe_thresh` | `Option<u32>` | フレームドロップ閾値 (0-100) |
| `rc_2pass_vbr_bias_pct` | `Option<u32>` | 2 パス CBR/VBR バイアス (0-100) |
| `rc_2pass_vbr_minsection_pct` | `Option<u32>` | 2 パス GOP 最小ビットレート (%) |
| `rc_2pass_vbr_maxsection_pct` | `Option<u32>` | 2 パス GOP 最大ビットレート (%) |

#### リサイズ / スーパーレゾリューション

| フィールド | 型 | 説明 |
|---|---|---|
| `rc_resize_mode` | `Option<u32>` | 空間リサンプリングモード (0-2) |
| `rc_resize_denominator` | `Option<u32>` | リサイズ分母 (8-16) |
| `rc_resize_kf_denominator` | `Option<u32>` | キーフレームリサイズ分母 (8-16) |
| `rc_superres_mode` | `Option<u32>` | スーパーレゾリューションモード (0-4) |
| `rc_superres_denominator` | `Option<u32>` | スーパーレゾリューション分母 (8-16) |
| `rc_superres_kf_denominator` | `Option<u32>` | キーフレームスーパーレゾリューション分母 (8-16) |
| `rc_superres_qthresh` | `Option<u32>` | スーパーレゾリューション Q 閾値 (1-63) |
| `rc_superres_kf_qthresh` | `Option<u32>` | キーフレームスーパーレゾリューション Q 閾値 (1-63) |

#### キーフレーム / GOP

| フィールド | 型 | 説明 |
|---|---|---|
| `kf_mode` | `Option<KeyframeMode>` | キーフレーム配置モード |
| `kf_min_dist` | `Option<u32>` | キーフレーム最小間隔 |
| `kf_max_dist` | `Option<u32>` | キーフレーム最大間隔 |
| `fwd_kf_enabled` | `Option<bool>` | 前方参照キーフレーム有効化 |
| `sframe_dist` | `Option<u32>` | S-Frame 間隔 |
| `sframe_mode` | `Option<u32>` | S-Frame 挿入モード (1 or 2) |

#### タイル設定

| フィールド | 型 | 説明 |
|---|---|---|
| `tile_columns` | `Option<i32>` | タイル列数 (log2) |
| `tile_rows` | `Option<i32>` | タイル行数 (log2) |
| `tile_width_count` | `Option<i32>` | 明示的タイル幅の数 |
| `tile_height_count` | `Option<i32>` | 明示的タイル高さの数 |
| `tile_widths` | `Option<Vec<i32>>` | タイル幅の配列 (最大 64) |
| `tile_heights` | `Option<Vec<i32>>` | タイル高さの配列 (最大 64) |
| `large_scale_tile` | `Option<bool>` | 大規模タイルモード |

#### エンコード速度 / 品質

| フィールド | 型 | 説明 |
|---|---|---|
| `cpu_used` | `Option<i32>` | エンコード速度 (0-10, 大きいほど高速) |
| `cq_level` | `Option<u32>` | CQ レベル |
| `sharpness` | `Option<u32>` | シャープネス (0-7) |
| `static_threshold` | `Option<u32>` | 静止検出閾値 |
| `arnr_max_frames` | `Option<u32>` | ARNR 最大フレーム数 |
| `arnr_strength` | `Option<u32>` | ARNR 強度 |
| `max_intra_bitrate_pct` | `Option<u32>` | I フレーム最大ビットレート (%) |
| `aq_mode` | `Option<u32>` | 適応的量子化モード (0-3) |
| `deltaq_mode` | `Option<u32>` | デルタ Q モード |
| `lossless` | `Option<bool>` | ロスレスモード |
| `noise_sensitivity` | `Option<u32>` | ノイズ感度 |

#### フィルター制御

| フィールド | 型 | 説明 |
|---|---|---|
| `enable_cdef` | `Option<bool>` | CDEF 有効化 |
| `enable_restoration` | `Option<bool>` | 復元フィルタ有効化 |
| `loopfilter_control` | `Option<u32>` | ループフィルタ制御 |
| `enable_obmc` | `Option<bool>` | OBMC 有効化 |
| `denoise_noise_level` | `Option<u32>` | デノイズノイズレベル |
| `denoise_block_size` | `Option<u32>` | デノイズブロックサイズ |
| `film_grain_test_vector` | `Option<u32>` | フィルムグレインテストベクタ |

#### モーション / 予測

| フィールド | 型 | 説明 |
|---|---|---|
| `enable_global_motion` | `Option<bool>` | グローバルモーション有効化 |
| `enable_warped_motion` | `Option<bool>` | ワープモーション有効化 |
| `enable_tpl_model` | `Option<bool>` | TPL モデル有効化 |
| `enable_keyframe_filtering` | `Option<u32>` | キーフレームフィルタリング (0-2) |
| `min_gf_interval` | `Option<u32>` | 最小 GF 間隔 |
| `max_gf_interval` | `Option<u32>` | 最大 GF 間隔 |
| `gf_max_pyramid_height` | `Option<u32>` | GF 最大ピラミッド高さ |
| `max_reference_frames` | `Option<u32>` | 最大参照フレーム数 |

#### Intra 予測

| フィールド | 型 | 説明 |
|---|---|---|
| `enable_filter_intra` | `Option<bool>` | フィルタ Intra 有効化 |
| `enable_smooth_intra` | `Option<bool>` | スムース Intra 有効化 |
| `enable_paeth_intra` | `Option<bool>` | Paeth Intra 有効化 |
| `enable_cfl_intra` | `Option<bool>` | CfL Intra 有効化 |
| `enable_palette` | `Option<bool>` | パレットモード有効化 |
| `enable_intrabc` | `Option<bool>` | IntraBC 有効化 |

#### パーティション

| フィールド | 型 | 説明 |
|---|---|---|
| `superblock_size` | `Option<SuperblockSize>` | スーパーブロックサイズ |
| `enable_rect_partitions` | `Option<bool>` | 矩形パーティション有効化 |
| `enable_ab_partitions` | `Option<bool>` | AB パーティション有効化 |
| `enable_1to4_partitions` | `Option<bool>` | 1:4 パーティション有効化 |

#### カラー情報

| フィールド | 型 | 説明 |
|---|---|---|
| `color_primaries` | `Option<u32>` | 色域 (CICP) |
| `transfer_characteristics` | `Option<u32>` | 伝送特性 (CICP) |
| `matrix_coefficients` | `Option<u32>` | 行列係数 (CICP) |
| `color_range` | `Option<u32>` | 色範囲 (0: スタジオ, 1: フル) |
| `enable_chroma_deltaq` | `Option<bool>` | クロマデルタ Q 有効化 |
| `enable_dual_filter` | `Option<bool>` | デュアルフィルタ有効化 |

#### その他

| フィールド | 型 | 説明 |
|---|---|---|
| `row_mt` | `Option<bool>` | 行マルチスレッド |
| `tune_content` | `Option<ContentType>` | コンテンツタイプ最適化 |
| `monochrome` | `Option<bool>` | モノクロモード |
| `full_still_picture_hdr` | `Option<bool>` | スティルピクチャ用フルヘッダ |
| `save_as_annexb` | `Option<bool>` | Annex-B 形式 |
| `use_fixed_qp_offsets` | `Option<bool>` | 固定 QP オフセット使用 |
| `enable_superres` | `Option<bool>` | スーパーレゾリューション有効化 |

### `Usage`

| バリアント | 説明 |
|---|---|
| `GoodQuality` | 高品質 (品質と速度のバランス) |
| `Realtime` | リアルタイム (最も高速) |
| `AllIntra` | 全 I フレーム (キーフレームのみ) |

### `RateControlMode`

| バリアント | 説明 |
|---|---|
| `Vbr` | Variable Bitrate (可変ビットレート) |
| `Cbr` | Constant Bitrate (固定ビットレート) |
| `Cq` | Constrained Quality (制約付き品質) |
| `Q` | Constant Quality (固定品質) |

### `ImageFormat`

| バリアント | 説明 |
|---|---|
| `I420` | YUV 4:2:0 planar (3 プレーン: Y, U, V) |
| `Yv12` | YUV 4:2:0 planar (3 プレーン: Y, V, U) |
| `Nv12` | YUV 4:2:0 semi-planar (2 プレーン: Y, UV interleaved) |
| `I422` | YUV 4:2:2 planar (3 プレーン: Y, U, V) |
| `I444` | YUV 4:4:4 planar (3 プレーン: Y, U, V) |
| `I42016` | YUV 4:2:0 planar 16-bit (3 プレーン: Y, U, V) |
| `I42216` | YUV 4:2:2 planar 16-bit (3 プレーン: Y, U, V) |
| `I44416` | YUV 4:4:4 planar 16-bit (3 プレーン: Y, U, V) |

### `ImageData`

| バリアント | 説明 |
|---|---|
| `I420 { y, u, v }` | I420 (3 プレーン: Y, U, V) |
| `Yv12 { y, u, v }` | YV12 (3 プレーン: Y, V, U) |
| `Nv12 { y, uv }` | NV12 (2 プレーン: Y, UV interleaved) |
| `I422 { y, u, v }` | I422 (3 プレーン: Y, U, V) |
| `I444 { y, u, v }` | I444 (3 プレーン: Y, U, V) |
| `I42016 { y, u, v }` | I42016 (3 プレーン: Y, U, V / 16-bit) |
| `I42216 { y, u, v }` | I42216 (3 プレーン: Y, U, V / 16-bit) |
| `I44416 { y, u, v }` | I44416 (3 プレーン: Y, U, V / 16-bit) |

### `SuperblockSize`

| バリアント | 説明 |
|---|---|
| `Size64x64` | 64x64 |
| `Size128x128` | 128x128 |
| `Dynamic` | 動的選択 |

### `ContentType`

| バリアント | 説明 |
|---|---|
| `Default` | 通常の映像 |
| `Screen` | スクリーン録画 |
| `Film` | フィルム |

### `KeyframeMode`

| バリアント | 説明 |
|---|---|
| `Fixed` | 固定間隔 |
| `Auto` | 自動配置 |

### `EncodingPass`

| バリアント | 説明 |
|---|---|
| `OnePass` | シングルパス |
| `FirstPass` | 1 パス目 |
| `SecondPass` | 2 パス目 |
| `ThirdPass` | 3 パス目 |

### `EncodeOptions`

| フィールド | 型 | デフォルト | 説明 |
|---|---|---|---|
| `force_keyframe` | `bool` | false | キーフレームを強制する |

### `DecoderConfig`

| フィールド | 型 | 説明 |
|---|---|---|
| `threads` | `Option<u32>` | スレッド数 |
| `w` | `Option<u32>` | 幅のヒント |
| `h` | `Option<u32>` | 高さのヒント |
| `allow_lowbitdepth` | `Option<bool>` | 低ビット深度パスの許可 |

## 環境変数

| 変数 | 説明 |
|---|---|
| `LIBAOM_TARGET` | prebuilt バイナリのプラットフォーム名を明示的に指定する |

## libaom ライセンス

<https://aomedia.googlesource.com/aom/+/refs/heads/main/LICENSE>

```text
Copyright (c) 2016, Alliance for Open Media. All rights reserved.

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions
are met:

1. Redistributions of source code must retain the above copyright
   notice, this list of conditions and the following disclaimer.

2. Redistributions in binary form must reproduce the above copyright
   notice, this list of conditions and the following disclaimer in
   the documentation and/or other materials provided with the
   distribution.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS
"AS IS" AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT
LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS
FOR A PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL THE
COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT,
INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING,
BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES;
LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER
CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT
LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN
ANY WAY OUT OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE
POSSIBILITY OF SUCH DAMAGE.
```

## ライセンス

Apache License 2.0

```text
Copyright 2026-2026, Shiguredo Inc.

Licensed under the Apache License, Version 2.0 (the "License");
you may not use this file except in compliance with the License.
You may obtain a copy of the License at

    http://www.apache.org/licenses/LICENSE-2.0

Unless required by applicable law or agreed to in writing, software
distributed under the License is distributed on an "AS IS" BASIS,
WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
See the License for the specific language governing permissions and
limitations under the License.
```
