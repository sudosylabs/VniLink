//! [libaom] (AV1) エンコーダーとデコーダーの Rust バインディング
//!
//! [libaom]: https://aomedia.googlesource.com/aom
#![warn(missing_docs)]

use std::{
    ffi::{CStr, c_int, c_uint},
    mem::MaybeUninit,
};

mod sys;

/// ビルド時に参照したリポジトリ URL
pub const BUILD_REPOSITORY: &str = sys::BUILD_METADATA_REPOSITORY;

/// ビルド時に参照したリポジトリのバージョン（タグ）
pub const BUILD_VERSION: &str = sys::BUILD_METADATA_VERSION;

/// エラー
#[derive(Debug)]
pub struct Error {
    code: sys::aom_codec_err_t,
    function: &'static str,
    reason: Option<&'static str>,
    detail: Option<String>,
}

impl Error {
    fn check(
        code: sys::aom_codec_err_t,
        function: &'static str,
        ctx: Option<&sys::aom_codec_ctx>,
    ) -> Result<(), Self> {
        if code == sys::aom_codec_err_t_AOM_CODEC_OK {
            Ok(())
        } else {
            let detail = unsafe {
                if let Some(ctx) = ctx {
                    let detail_ptr = sys::aom_codec_error_detail(ctx);
                    if detail_ptr.is_null() {
                        None
                    } else {
                        CStr::from_ptr(detail_ptr)
                            .to_str()
                            .ok()
                            .map(|s| s.to_owned())
                    }
                } else {
                    None
                }
            };
            Err(Self {
                code,
                function,
                reason: None,
                detail,
            })
        }
    }

    fn with_reason(
        code: sys::aom_codec_err_t,
        function: &'static str,
        reason: &'static str,
    ) -> Self {
        Self {
            code,
            function,
            reason: Some(reason),
            detail: None,
        }
    }

    fn reason(&self) -> Option<&str> {
        if self.reason.is_some() {
            return self.reason;
        }

        let reason = unsafe { sys::aom_codec_err_to_string(self.code) };
        if reason.is_null() {
            None
        } else {
            unsafe { CStr::from_ptr(reason) }.to_str().ok()
        }
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}() failed: code={}", self.function, self.code)?;
        if let Some(reason) = self.reason() {
            write!(f, ", reason={reason}")?;
        }
        if let Some(detail) = &self.detail {
            write!(f, ", detail={detail}")?;
        }
        Ok(())
    }
}

impl std::error::Error for Error {}

// ============================================================================
// デコーダー設定
// ============================================================================

/// デコーダーに指定する設定
///
/// フィールド名は libaom の `aom_codec_dec_cfg_t` に準拠する。
/// すべて `Option` で、`None` の場合は libaom のデフォルト値が使われる。
#[derive(Debug, Clone)]
pub struct DecoderConfig {
    /// スレッド数 (デフォルト: 1)
    pub threads: Option<u32>,

    /// 幅のヒント
    pub w: Option<u32>,

    /// 高さのヒント
    pub h: Option<u32>,

    /// 低ビット深度パスの使用を許可する
    pub allow_lowbitdepth: Option<bool>,
}

impl DecoderConfig {
    /// デコーダー設定を生成する
    pub fn new() -> Self {
        Self {
            threads: None,
            w: None,
            h: None,
            allow_lowbitdepth: None,
        }
    }
}

impl Default for DecoderConfig {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// デコーダー
// ============================================================================

/// AV1 デコーダー
pub struct Decoder {
    ctx: sys::aom_codec_ctx,
    iter: sys::aom_codec_iter_t,
}

impl Decoder {
    /// デコーダーインスタンスを生成する
    pub fn new(config: DecoderConfig) -> Result<Self, Error> {
        unsafe {
            let iface = sys::aom_codec_av1_dx();
            Self::init(iface, &config)
        }
    }

    fn init(iface: *const sys::aom_codec_iface, config: &DecoderConfig) -> Result<Self, Error> {
        let mut ctx = MaybeUninit::<sys::aom_codec_ctx>::zeroed();

        // デコーダー設定が指定されている場合は aom_codec_dec_cfg を構築する
        //
        // cfg は aom_codec_dec_init_ver に渡すまでスタック上に存在する必要があるため、
        // if ブロックの外で変数を保持する。
        let has_config = config.threads.is_some()
            || config.w.is_some()
            || config.h.is_some()
            || config.allow_lowbitdepth.is_some();

        let cfg = sys::aom_codec_dec_cfg {
            threads: config.threads.unwrap_or(1) as c_uint,
            w: config.w.unwrap_or(0) as c_uint,
            h: config.h.unwrap_or(0) as c_uint,
            allow_lowbitdepth: if config.allow_lowbitdepth.unwrap_or(false) {
                1
            } else {
                0
            },
        };
        let cfg_ptr = if has_config {
            &cfg as *const sys::aom_codec_dec_cfg
        } else {
            std::ptr::null()
        };

        unsafe {
            let code = sys::aom_codec_dec_init_ver(
                ctx.as_mut_ptr(),
                iface,
                cfg_ptr,
                0, // flags
                sys::AOM_DECODER_ABI_VERSION as i32,
            );
            // 初期化失敗時は ctx が未初期化なので参照してはいけない
            Error::check(code, "aom_codec_dec_init_ver", None)?;
            let ctx = ctx.assume_init();

            Ok(Self {
                ctx,
                iter: std::ptr::null(),
            })
        }
    }

    /// 圧縮された映像フレームをデコードする
    ///
    /// デコード結果は [`Decoder::next_frame()`] で取得できる
    pub fn decode(&mut self, data: &[u8]) -> Result<(), Error> {
        if !self.iter.is_null() {
            return Err(Error::with_reason(
                sys::aom_codec_err_t_AOM_CODEC_ERROR,
                "shiguredo_aom::Decoder::decode",
                "still need to call shiguredo_aom::Decoder::next_frame()",
            ));
        }

        let code = unsafe {
            sys::aom_codec_decode(
                &mut self.ctx,
                data.as_ptr(),
                data.len(),
                std::ptr::null_mut(), // user_priv
            )
        };
        Error::check(code, "aom_codec_decode", Some(&self.ctx))?;
        Ok(())
    }

    /// これ以上データが来ないことをデコーダーに伝える
    ///
    /// 残りのデコード結果は [`Decoder::next_frame()`] で取得できる
    pub fn finish(&mut self) -> Result<(), Error> {
        if !self.iter.is_null() {
            return Err(Error::with_reason(
                sys::aom_codec_err_t_AOM_CODEC_ERROR,
                "shiguredo_aom::Decoder::finish",
                "still need to call shiguredo_aom::Decoder::next_frame()",
            ));
        }

        let code = unsafe {
            sys::aom_codec_decode(&mut self.ctx, std::ptr::null_mut(), 0, std::ptr::null_mut())
        };
        Error::check(code, "aom_codec_decode", Some(&self.ctx))?;
        Ok(())
    }

    /// デコード済みのフレームを取り出す
    ///
    /// [`Decoder::decode()`] や [`Decoder::finish()`] の後には、
    /// このメソッドを、結果が `None` になるまで呼び出し続ける必要がある
    pub fn next_frame(&mut self) -> Option<DecodedFrame<'_>> {
        unsafe {
            let image = sys::aom_codec_get_frame(&mut self.ctx, &mut self.iter);
            if image.is_null() {
                self.iter = std::ptr::null();
                return None;
            }
            let image = &*image;

            Some(DecodedFrame(image))
        }
    }
}

// 安全性: aom_codec_ctx はスレッド間で移動しても安全である。
// libaom の内部状態はスレッドローカルな資源に依存せず、
// コンテキストへの排他的アクセスがあれば（&mut self で保証される）問題ない。
unsafe impl Send for Decoder {}

impl Drop for Decoder {
    fn drop(&mut self) {
        unsafe {
            sys::aom_codec_destroy(&mut self.ctx);
        }
    }
}

impl std::fmt::Debug for Decoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Decoder").finish_non_exhaustive()
    }
}

/// デコードされた映像フレーム
///
/// libaom のデコーダーはビットストリームのプロファイルに応じて
/// I420, I422, I444 (および各 16-bit 版) のいずれかを返す。
/// フォーマットの自動変換は行われない。
pub struct DecodedFrame<'a>(&'a sys::aom_image);

impl DecodedFrame<'_> {
    /// デコードされたフレームの画像フォーマットを返す
    ///
    /// libaom が未知のフォーマットを返した場合はエラーを返す
    pub fn format(&self) -> Result<ImageFormat, Error> {
        match self.0.fmt {
            sys::aom_img_fmt_AOM_IMG_FMT_I420 => Ok(ImageFormat::I420),
            sys::aom_img_fmt_AOM_IMG_FMT_I422 => Ok(ImageFormat::I422),
            sys::aom_img_fmt_AOM_IMG_FMT_I444 => Ok(ImageFormat::I444),
            sys::aom_img_fmt_AOM_IMG_FMT_I42016 => Ok(ImageFormat::I42016),
            sys::aom_img_fmt_AOM_IMG_FMT_I42216 => Ok(ImageFormat::I42216),
            sys::aom_img_fmt_AOM_IMG_FMT_I44416 => Ok(ImageFormat::I44416),
            _ => Err(Error::with_reason(
                sys::aom_codec_err_t_AOM_CODEC_ERROR,
                "shiguredo_aom::DecodedFrame::format",
                "unexpected image format from libaom decoder",
            )),
        }
    }

    /// フレームが高ビット深度（16ビット）かどうかを返す
    //
    // libaom での高ビット深度フォーマットについてのメモ：
    // - libaom は AV1 の 10-bit プロファイル（Profile 0 の 10-bit など）をサポート
    // - 高ビット深度データは 16-bit リトルエンディアン形式で格納される
    // - 実際の値範囲は 10-bit (0-1023) だが、上位6ビットは未使用
    // - ストライドは 16-bit 単位（バイト数は width * 2）で計算される
    pub fn is_high_depth(&self) -> bool {
        matches!(
            self.0.fmt,
            sys::aom_img_fmt_AOM_IMG_FMT_I42016
                | sys::aom_img_fmt_AOM_IMG_FMT_I42216
                | sys::aom_img_fmt_AOM_IMG_FMT_I44416
        )
    }

    /// UV プレーンの高さを返す
    ///
    /// 4:2:0 系は Y の半分、4:2:2 系と 4:4:4 系は Y と同じ。
    fn uv_height(&self) -> usize {
        match self.0.fmt {
            sys::aom_img_fmt_AOM_IMG_FMT_I420 | sys::aom_img_fmt_AOM_IMG_FMT_I42016 => {
                self.0.d_h.div_ceil(2) as usize
            }
            _ => self.0.d_h as usize,
        }
    }

    /// プレーンのデータをスライスとして返す
    ///
    /// stride が正でない、または planes ポインタが NULL の場合はエラーを返す
    fn plane(&self, index: usize, height: usize) -> Result<&[u8], Error> {
        let ptr = self.0.planes[index];
        if ptr.is_null() {
            return Err(Error::with_reason(
                sys::aom_codec_err_t_AOM_CODEC_ERROR,
                "shiguredo_aom::DecodedFrame::plane",
                "plane pointer is null",
            ));
        }
        let stride = self.0.stride[index];
        if stride <= 0 {
            return Err(Error::with_reason(
                sys::aom_codec_err_t_AOM_CODEC_ERROR,
                "shiguredo_aom::DecodedFrame::plane",
                "plane stride is not positive",
            ));
        }
        let stride_usize = stride as usize;
        let len = height.checked_mul(stride_usize).ok_or_else(|| {
            Error::with_reason(
                sys::aom_codec_err_t_AOM_CODEC_ERROR,
                "shiguredo_aom::DecodedFrame::plane",
                "plane size overflow: height * stride exceeds usize",
            )
        })?;
        if len > isize::MAX as usize {
            return Err(Error::with_reason(
                sys::aom_codec_err_t_AOM_CODEC_ERROR,
                "shiguredo_aom::DecodedFrame::plane",
                "plane size exceeds isize::MAX",
            ));
        }
        Ok(unsafe { std::slice::from_raw_parts(ptr, len) })
    }

    /// フレームの Y 成分のデータを返す
    pub fn y_plane(&self) -> Result<&[u8], Error> {
        self.plane(0, self.0.d_h as usize)
    }

    /// フレームの U 成分のデータを返す
    pub fn u_plane(&self) -> Result<&[u8], Error> {
        self.plane(1, self.uv_height())
    }

    /// フレームの V 成分のデータを返す
    pub fn v_plane(&self) -> Result<&[u8], Error> {
        self.plane(2, self.uv_height())
    }

    /// プレーンのストライドを返す
    ///
    /// stride が正でない場合はエラーを返す
    fn stride(&self, index: usize) -> Result<usize, Error> {
        let stride = self.0.stride[index];
        if stride <= 0 {
            return Err(Error::with_reason(
                sys::aom_codec_err_t_AOM_CODEC_ERROR,
                "shiguredo_aom::DecodedFrame::stride",
                "plane stride is not positive",
            ));
        }
        Ok(stride as usize)
    }

    /// フレームの Y 成分のストライドを返す
    pub fn y_stride(&self) -> Result<usize, Error> {
        self.stride(0)
    }

    /// フレームの U 成分のストライドを返す
    pub fn u_stride(&self) -> Result<usize, Error> {
        self.stride(1)
    }

    /// フレームの V 成分のストライドを返す
    pub fn v_stride(&self) -> Result<usize, Error> {
        self.stride(2)
    }

    /// フレームの幅を返す
    pub fn width(&self) -> usize {
        self.0.d_w as usize
    }

    /// フレームの高さを返す
    pub fn height(&self) -> usize {
        self.0.d_h as usize
    }

    /// The matrix coefficients the sequence header declares (CICP; 2 when
    /// it declares none).
    pub fn matrix_coefficients(&self) -> u32 {
        self.0.mc as u32
    }

    /// Whether the sequence header declares full range rather than studio
    /// range.
    pub fn full_range(&self) -> bool {
        self.0.range == sys::aom_color_range_AOM_CR_FULL_RANGE
    }
}

// ============================================================================
// 画像フォーマット / 画像データ
// ============================================================================

/// エンコーダーの入力画像フォーマット
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageFormat {
    /// YUV 4:2:0 planar (3 プレーン: Y, U, V)
    I420,
    /// YUV 4:2:0 planar (3 プレーン: Y, V, U)
    Yv12,
    /// YUV 4:2:0 semi-planar (2 プレーン: Y, UV interleaved)
    Nv12,
    /// YUV 4:2:2 planar (3 プレーン: Y, U, V)
    I422,
    /// YUV 4:4:4 planar (3 プレーン: Y, U, V)
    I444,
    /// YUV 4:2:0 planar 16-bit (3 プレーン: Y, U, V)
    I42016,
    /// YUV 4:2:2 planar 16-bit (3 プレーン: Y, U, V)
    I42216,
    /// YUV 4:4:4 planar 16-bit (3 プレーン: Y, U, V)
    I44416,
}

/// エンコーダーに渡す画像データ
pub enum ImageData<'a> {
    /// I420 (3 プレーン: Y, U, V)
    I420 {
        /// Y プレーン
        y: &'a [u8],
        /// U プレーン
        u: &'a [u8],
        /// V プレーン
        v: &'a [u8],
    },
    /// YV12 (3 プレーン: Y, V, U)
    Yv12 {
        /// Y プレーン
        y: &'a [u8],
        /// U プレーン
        u: &'a [u8],
        /// V プレーン
        v: &'a [u8],
    },
    /// NV12 (2 プレーン: Y, UV interleaved)
    Nv12 {
        /// Y プレーン
        y: &'a [u8],
        /// UV interleaved プレーン
        uv: &'a [u8],
    },
    /// I422 (3 プレーン: Y, U, V)
    I422 {
        /// Y プレーン
        y: &'a [u8],
        /// U プレーン
        u: &'a [u8],
        /// V プレーン
        v: &'a [u8],
    },
    /// I444 (3 プレーン: Y, U, V)
    I444 {
        /// Y プレーン
        y: &'a [u8],
        /// U プレーン
        u: &'a [u8],
        /// V プレーン
        v: &'a [u8],
    },
    /// I42016 (3 プレーン: Y, U, V / 16-bit)
    I42016 {
        /// Y プレーン
        y: &'a [u8],
        /// U プレーン
        u: &'a [u8],
        /// V プレーン
        v: &'a [u8],
    },
    /// I42216 (3 プレーン: Y, U, V / 16-bit)
    I42216 {
        /// Y プレーン
        y: &'a [u8],
        /// U プレーン
        u: &'a [u8],
        /// V プレーン
        v: &'a [u8],
    },
    /// I44416 (3 プレーン: Y, U, V / 16-bit)
    I44416 {
        /// Y プレーン
        y: &'a [u8],
        /// U プレーン
        u: &'a [u8],
        /// V プレーン
        v: &'a [u8],
    },
}

impl ImageData<'_> {
    /// この画像データに対応するフォーマットを返す
    fn format(&self) -> ImageFormat {
        match self {
            ImageData::I420 { .. } => ImageFormat::I420,
            ImageData::Yv12 { .. } => ImageFormat::Yv12,
            ImageData::Nv12 { .. } => ImageFormat::Nv12,
            ImageData::I422 { .. } => ImageFormat::I422,
            ImageData::I444 { .. } => ImageFormat::I444,
            ImageData::I42016 { .. } => ImageFormat::I42016,
            ImageData::I42216 { .. } => ImageFormat::I42216,
            ImageData::I44416 { .. } => ImageFormat::I44416,
        }
    }
}

/// 各プレーンの期待サイズ
enum PlaneSizes {
    /// 3 プレーン (I420, YV12, I422, I444, I42016, I42216, I44416)
    ThreePlanes {
        y_size: usize,
        u_size: usize,
        v_size: usize,
    },
    /// 2 プレーン (NV12)
    TwoPlanes { y_size: usize, uv_size: usize },
}

// ============================================================================
// エンコーダー列挙型
// ============================================================================

/// エンコーダーの利用モード (g_usage)
///
/// `aom_codec_enc_config_default()` の `usage` パラメータでモードを指定する。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Usage {
    /// 高品質 (品質と速度のバランス)
    GoodQuality,
    /// リアルタイム (最も高速)
    Realtime,
    /// 全 I フレーム (キーフレームのみ)
    AllIntra,
}

/// レート制御モード (rc_end_usage)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateControlMode {
    /// Variable Bitrate (可変ビットレート)
    Vbr,
    /// Constant Bitrate (固定ビットレート)
    Cbr,
    /// Constrained Quality (制約付き品質)
    Cq,
    /// Constant Quality (固定品質)
    Q,
}

/// キーフレーム配置モード (kf_mode)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyframeMode {
    /// 固定間隔
    Fixed,
    /// 自動配置
    Auto,
}

/// マルチパスエンコーディングモード (g_pass)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncodingPass {
    /// シングルパス
    OnePass,
    /// 1 パス目
    FirstPass,
    /// 2 パス目
    SecondPass,
    /// 3 パス目
    ThirdPass,
}

/// タイムベース (aom_rational)
///
/// libaom の `aom_rational` 構造体に対応する。
/// タイムベースはストリームの最小時間単位を秒で表す。
/// 例: 30fps の場合、`num = 1`, `den = 30`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AomRational {
    /// 分子
    pub num: i32,
    /// 分母
    pub den: i32,
}

/// スーパーブロックサイズ (AV1E_SET_SUPERBLOCK_SIZE)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SuperblockSize {
    /// 64x64
    Size64x64,
    /// 128x128
    Size128x128,
    /// 動的選択
    Dynamic,
}

/// コンテンツタイプ (AV1E_SET_TUNE_CONTENT)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentType {
    /// 通常の映像
    Default,
    /// スクリーン録画
    Screen,
    /// フィルム
    Film,
}

// ============================================================================
// コーデック対応情報
// ============================================================================

/// コーデック種別
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoCodecType {
    /// AV1
    Av1,
}

/// AV1 エンコーディングプロファイル
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Av1EncodingProfile {
    /// Profile 0: 8/10-bit 4:2:0
    Profile0,
    /// Profile 1: 8/10-bit 4:4:4
    Profile1,
    /// Profile 2: 8/10/12-bit 4:2:0, 4:2:2, 4:4:4
    Profile2,
}

/// コーデック固有のエンコードプロファイル情報
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EncodingProfiles {
    /// AV1 プロファイル一覧
    Av1(Vec<Av1EncodingProfile>),
    /// プロファイル情報なし（プロファイルの概念がないコーデック向け）
    Unsupported,
}

/// デコード対応情報
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodingInfo {
    /// デコードに対応しているか
    pub supported: bool,
    /// ハードウェアアクセラレーションに対応しているか
    pub hardware_accelerated: bool,
}

/// エンコード対応情報
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodingInfo {
    /// エンコードに対応しているか
    pub supported: bool,
    /// ハードウェアアクセラレーションに対応しているか
    pub hardware_accelerated: bool,
    /// コーデック固有のプロファイル情報
    pub profiles: EncodingProfiles,
}

/// コーデック対応情報
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodecInfo {
    /// コーデック種別
    pub codec: VideoCodecType,
    /// デコード情報
    pub decoding: DecodingInfo,
    /// エンコード情報
    pub encoding: EncodingInfo,
}

/// 利用可能な AV1 コーデックの対応情報を返す
///
/// libaom はソフトウェアコーデックであるため、`hardware_accelerated` は常に `false` を返す。
/// プロファイルの照会は 1920x1080 の解像度で行う。
pub fn supported_codecs() -> CodecInfo {
    let decoding = DecodingInfo {
        supported: !unsafe { sys::aom_codec_av1_dx() }.is_null(),
        hardware_accelerated: false,
    };

    let encoder_available = !unsafe { sys::aom_codec_av1_cx() }.is_null();

    let profiles = if encoder_available {
        EncodingProfiles::Av1(detect_supported_profiles())
    } else {
        EncodingProfiles::Unsupported
    };

    let encoding = EncodingInfo {
        supported: encoder_available,
        hardware_accelerated: false,
        profiles,
    };

    CodecInfo {
        codec: VideoCodecType::Av1,
        decoding,
        encoding,
    }
}

/// エンコーダーの初期化を試行してプロファイルの対応状況を判定する
fn detect_supported_profiles() -> Vec<Av1EncodingProfile> {
    let candidates = [
        (0, Av1EncodingProfile::Profile0),
        (1, Av1EncodingProfile::Profile1),
        (2, Av1EncodingProfile::Profile2),
    ];

    let mut profiles = Vec::new();

    for (profile_id, profile) in candidates {
        if is_profile_supported(profile_id) {
            profiles.push(profile);
        }
    }

    profiles
}

/// 指定したプロファイルでエンコーダーを初期化できるか試行する
fn is_profile_supported(profile_id: u32) -> bool {
    unsafe {
        let iface = sys::aom_codec_av1_cx();
        if iface.is_null() {
            return false;
        }

        let mut cfg = MaybeUninit::<sys::aom_codec_enc_cfg>::zeroed();
        let code =
            sys::aom_codec_enc_config_default(iface, cfg.as_mut_ptr(), sys::AOM_USAGE_GOOD_QUALITY);
        if code != sys::aom_codec_err_t_AOM_CODEC_OK {
            return false;
        }

        let mut cfg = cfg.assume_init();
        cfg.g_profile = profile_id;
        cfg.g_w = 1920;
        cfg.g_h = 1080;

        let mut ctx = MaybeUninit::<sys::aom_codec_ctx>::zeroed();
        let code = sys::aom_codec_enc_init_ver(
            ctx.as_mut_ptr(),
            iface,
            &cfg,
            0,
            sys::AOM_ENCODER_ABI_VERSION as i32,
        );

        if code == sys::aom_codec_err_t_AOM_CODEC_OK {
            let mut ctx = ctx.assume_init();
            sys::aom_codec_destroy(&mut ctx);
            true
        } else {
            false
        }
    }
}

// ============================================================================
// エンコーダー設定
// ============================================================================

/// エンコーダーに指定する設定
///
/// フィールド名は libaom の `aom_codec_enc_cfg_t` および
/// エンコーダー制御パラメータ (`aom_codec_control`) に準拠する。
///
/// `Option` のフィールドは `None` の場合、libaom のデフォルト値がそのまま使われる。
#[derive(Debug, Clone)]
pub struct EncoderConfig {
    // --- 入力画像設定 (libaom 外) ---
    /// 入力画像フォーマット
    pub image_format: ImageFormat,

    // ========================================================================
    // aom_codec_enc_cfg_t フィールド
    // ========================================================================

    // --- 一般設定 (g_*) ---
    /// エンコーダーの利用モード
    pub g_usage: Usage,

    /// スレッド数 (0 は 1 と同等)
    pub g_threads: Option<u32>,

    /// ビットストリームプロファイル (0, 1, 2)
    pub g_profile: u32,

    /// フレームの幅
    pub g_w: u32,

    /// フレームの高さ
    pub g_h: u32,

    /// エンコードする最大フレーム数 (0 で無制限)
    pub g_limit: Option<u32>,

    /// 強制最大フレーム幅 (0 で無効)
    pub g_forced_max_frame_width: Option<u32>,

    /// 強制最大フレーム高さ (0 で無効)
    pub g_forced_max_frame_height: Option<u32>,

    /// コーデックのビット深度 (8, 10, 12)
    pub g_bit_depth: Option<u32>,

    /// 入力フレームのビット深度
    pub g_input_bit_depth: Option<u32>,

    /// タイムベース (例: 30fps なら num=1, den=30)
    pub g_timebase: AomRational,

    /// エラー耐性モード
    pub g_error_resilient: bool,

    /// マルチパスエンコーディングモード
    pub g_pass: Option<EncodingPass>,

    /// 先読みフレーム数 (0 で無効)
    pub g_lag_in_frames: Option<u32>,

    // --- レート制御 (rc_*) ---
    /// フレームドロップ閾値 (0-100, 0 で無効)
    pub rc_dropframe_thresh: Option<u32>,

    /// 空間リサンプリングモード (0: 無効, 1: 固定, 2: ランダム)
    pub rc_resize_mode: Option<u32>,

    /// フレームリサイズ分母 (8-16, 分子は 8)
    pub rc_resize_denominator: Option<u32>,

    /// キーフレームリサイズ分母 (8-16, 分子は 8)
    pub rc_resize_kf_denominator: Option<u32>,

    /// スーパーレゾリューションモード (0: 無効, 1: 固定, 2: ランダム, 3: Q閾値, 4: 自動)
    pub rc_superres_mode: Option<u32>,

    /// スーパーレゾリューション分母 (8-16)
    pub rc_superres_denominator: Option<u32>,

    /// キーフレームスーパーレゾリューション分母 (8-16)
    pub rc_superres_kf_denominator: Option<u32>,

    /// スーパーレゾリューション Q 閾値 (1-63)
    pub rc_superres_qthresh: Option<u32>,

    /// キーフレームスーパーレゾリューション Q 閾値 (1-63)
    pub rc_superres_kf_qthresh: Option<u32>,

    /// レート制御モード
    pub rc_end_usage: RateControlMode,

    /// ターゲットビットレート (kbps)
    pub rc_target_bitrate: u32,

    /// 最小量子化値 (最高品質)
    pub rc_min_quantizer: u32,

    /// 最大量子化値 (最低品質)
    pub rc_max_quantizer: u32,

    /// VBR アンダーシュート許容率 (0-100)
    pub rc_undershoot_pct: Option<u32>,

    /// VBR オーバーシュート許容率 (0-100)
    pub rc_overshoot_pct: Option<u32>,

    /// デコーダーバッファサイズ (ms)
    pub rc_buf_sz: Option<u32>,

    /// デコーダーバッファ初期サイズ (ms)
    pub rc_buf_initial_sz: Option<u32>,

    /// デコーダーバッファ最適サイズ (ms)
    pub rc_buf_optimal_sz: Option<u32>,

    /// 2 パス CBR/VBR バイアス (0-100, 0=CBR寄り, 100=VBR寄り)
    pub rc_2pass_vbr_bias_pct: Option<u32>,

    /// 2 パス GOP 最小ビットレート (ターゲットの%)
    pub rc_2pass_vbr_minsection_pct: Option<u32>,

    /// 2 パス GOP 最大ビットレート (ターゲットの%)
    pub rc_2pass_vbr_maxsection_pct: Option<u32>,

    // --- キーフレーム設定 (kf_*) ---
    /// 前方参照キーフレームの有効化
    pub fwd_kf_enabled: Option<bool>,

    /// キーフレーム配置モード
    pub kf_mode: Option<KeyframeMode>,

    /// キーフレーム最小間隔
    pub kf_min_dist: Option<u32>,

    /// キーフレーム最大間隔
    pub kf_max_dist: Option<u32>,

    // --- S-Frame 設定 ---
    /// S-Frame 間隔 (0 で無効)
    pub sframe_dist: Option<u32>,

    /// S-Frame 挿入モード (1 or 2)
    pub sframe_mode: Option<u32>,

    // --- タイルサイズ設定 ---
    /// 明示的タイル幅の数
    pub tile_width_count: Option<i32>,

    /// 明示的タイル高さの数
    pub tile_height_count: Option<i32>,

    /// タイル幅の配列 (最大 64 要素)
    pub tile_widths: Option<Vec<i32>>,

    /// タイル高さの配列 (最大 64 要素)
    pub tile_heights: Option<Vec<i32>>,

    // --- その他の cfg フィールド ---
    /// タイルコーディングモード (0: 通常, 1: 大規模タイル)
    pub large_scale_tile: Option<bool>,

    /// モノクロモード
    pub monochrome: Option<bool>,

    /// スティルピクチャ用フルヘッダ
    pub full_still_picture_hdr: Option<bool>,

    /// Annex-B 形式で保存 (0: Section 5, 1: Annex-B)
    pub save_as_annexb: Option<bool>,

    /// 固定 QP オフセットの使用
    pub use_fixed_qp_offsets: Option<bool>,

    // ========================================================================
    // エンコーダー制御パラメータ (aom_codec_control)
    // ========================================================================
    /// AOME_SET_CPUUSED: エンコード速度 (0-10, 大きいほど高速)
    pub cpu_used: Option<i32>,

    /// AOME_SET_CQ_LEVEL: CQ レベル
    pub cq_level: Option<u32>,

    /// AOME_SET_SHARPNESS: シャープネス (0-7)
    pub sharpness: Option<u32>,

    /// AOME_SET_STATIC_THRESHOLD: 静止検出閾値
    pub static_threshold: Option<u32>,

    /// AOME_SET_ARNR_MAXFRAMES: ARNR 最大フレーム数
    pub arnr_max_frames: Option<u32>,

    /// AOME_SET_ARNR_STRENGTH: ARNR 強度
    pub arnr_strength: Option<u32>,

    /// AOME_SET_MAX_INTRA_BITRATE_PCT: I フレーム最大ビットレート (ターゲットの%)
    pub max_intra_bitrate_pct: Option<u32>,

    /// AV1E_SET_LOSSLESS: ロスレスモード
    pub lossless: Option<bool>,

    /// AV1E_SET_ROW_MT: 行マルチスレッド
    pub row_mt: Option<bool>,

    /// AV1E_SET_TILE_COLUMNS: タイル列数 (log2)
    pub tile_columns: Option<i32>,

    /// AV1E_SET_TILE_ROWS: タイル行数 (log2)
    pub tile_rows: Option<i32>,

    /// AV1E_SET_ENABLE_TPL_MODEL: TPL モデル有効化
    pub enable_tpl_model: Option<bool>,

    /// AV1E_SET_ENABLE_KEYFRAME_FILTERING: キーフレームフィルタリング (0-2)
    pub enable_keyframe_filtering: Option<u32>,

    /// AV1E_SET_AQ_MODE: 適応的量子化モード (0-3)
    pub aq_mode: Option<u32>,

    /// AV1E_SET_DELTAQ_MODE: デルタ Q モード
    pub deltaq_mode: Option<u32>,

    /// AV1E_SET_NOISE_SENSITIVITY: ノイズ感度
    pub noise_sensitivity: Option<u32>,

    /// AV1E_SET_TUNE_CONTENT: コンテンツタイプ最適化
    pub tune_content: Option<ContentType>,

    /// AV1E_SET_COLOR_PRIMARIES: 色域 (CICP)
    pub color_primaries: Option<u32>,

    /// AV1E_SET_TRANSFER_CHARACTERISTICS: 伝送特性 (CICP)
    pub transfer_characteristics: Option<u32>,

    /// AV1E_SET_MATRIX_COEFFICIENTS: 行列係数 (CICP)
    pub matrix_coefficients: Option<u32>,

    /// AV1E_SET_COLOR_RANGE: 色範囲 (0: スタジオ, 1: フル)
    pub color_range: Option<u32>,

    /// AV1E_SET_SUPERBLOCK_SIZE: スーパーブロックサイズ
    pub superblock_size: Option<SuperblockSize>,

    /// AV1E_SET_ENABLE_CDEF: CDEF 有効化
    pub enable_cdef: Option<bool>,

    /// AV1E_SET_ENABLE_RESTORATION: 復元フィルタ有効化
    pub enable_restoration: Option<bool>,

    /// AV1E_SET_ENABLE_OBMC: OBMC 有効化
    pub enable_obmc: Option<bool>,

    /// AV1E_SET_ENABLE_GLOBAL_MOTION: グローバルモーション有効化
    pub enable_global_motion: Option<bool>,

    /// AV1E_SET_ENABLE_WARPED_MOTION: ワープモーション有効化
    pub enable_warped_motion: Option<bool>,

    /// AV1E_SET_ENABLE_PALETTE: パレットモード有効化
    pub enable_palette: Option<bool>,

    /// AV1E_SET_ENABLE_FILTER_INTRA: フィルタ Intra 有効化
    pub enable_filter_intra: Option<bool>,

    /// AV1E_SET_ENABLE_SMOOTH_INTRA: スムース Intra 有効化
    pub enable_smooth_intra: Option<bool>,

    /// AV1E_SET_ENABLE_PAETH_INTRA: Paeth Intra 有効化
    pub enable_paeth_intra: Option<bool>,

    /// AV1E_SET_ENABLE_CFL_INTRA: CfL Intra 有効化
    pub enable_cfl_intra: Option<bool>,

    /// AV1E_SET_MIN_GF_INTERVAL: 最小 GF 間隔
    pub min_gf_interval: Option<u32>,

    /// AV1E_SET_MAX_GF_INTERVAL: 最大 GF 間隔
    pub max_gf_interval: Option<u32>,

    /// AV1E_SET_DENOISE_NOISE_LEVEL: デノイズノイズレベル (0 で無効)
    pub denoise_noise_level: Option<u32>,

    /// AV1E_SET_DENOISE_BLOCK_SIZE: デノイズブロックサイズ
    pub denoise_block_size: Option<u32>,

    /// AV1E_SET_FILM_GRAIN_TEST_VECTOR: フィルムグレインテストベクタ (0 で無効)
    pub film_grain_test_vector: Option<u32>,

    /// AV1E_SET_LOOPFILTER_CONTROL: ループフィルタ制御
    pub loopfilter_control: Option<u32>,

    /// AV1E_SET_ENABLE_RECT_PARTITIONS: 矩形パーティション有効化
    pub enable_rect_partitions: Option<bool>,

    /// AV1E_SET_ENABLE_AB_PARTITIONS: AB パーティション有効化
    pub enable_ab_partitions: Option<bool>,

    /// AV1E_SET_ENABLE_1TO4_PARTITIONS: 1:4 パーティション有効化
    pub enable_1to4_partitions: Option<bool>,

    /// AV1E_SET_ENABLE_DUAL_FILTER: デュアルフィルタ有効化
    pub enable_dual_filter: Option<bool>,

    /// AV1E_SET_ENABLE_CHROMA_DELTAQ: クロマデルタ Q 有効化
    pub enable_chroma_deltaq: Option<bool>,

    /// AV1E_SET_ENABLE_INTRABC: IntraBC 有効化
    pub enable_intrabc: Option<bool>,

    /// AV1E_SET_ENABLE_SUPERRES: スーパーレゾリューション有効化
    pub enable_superres: Option<bool>,

    /// AV1E_SET_GF_MAX_PYRAMID_HEIGHT: GF 最大ピラミッド高さ
    pub gf_max_pyramid_height: Option<u32>,

    /// AV1E_SET_MAX_REFERENCE_FRAMES: 最大参照フレーム数
    pub max_reference_frames: Option<u32>,
}

impl EncoderConfig {
    /// 必須パラメータを指定してエンコーダー設定を生成する
    ///
    /// `g_w`, `g_h` はピクセル単位の幅と高さ。
    /// `rc_target_bitrate` は kbps 単位。
    /// その他のパラメータはデフォルト値で初期化される。
    pub fn new(g_w: u32, g_h: u32, image_format: ImageFormat) -> Self {
        Self {
            image_format,

            // aom_codec_enc_cfg_t
            g_usage: Usage::GoodQuality,
            g_threads: None,
            g_profile: 0,
            g_w,
            g_h,
            g_limit: None,
            g_forced_max_frame_width: None,
            g_forced_max_frame_height: None,
            g_bit_depth: None,
            g_input_bit_depth: None,
            g_timebase: AomRational { num: 1, den: 30 },
            g_error_resilient: false,
            g_pass: None,
            g_lag_in_frames: None,
            rc_dropframe_thresh: None,
            rc_resize_mode: None,
            rc_resize_denominator: None,
            rc_resize_kf_denominator: None,
            rc_superres_mode: None,
            rc_superres_denominator: None,
            rc_superres_kf_denominator: None,
            rc_superres_qthresh: None,
            rc_superres_kf_qthresh: None,
            rc_end_usage: RateControlMode::Vbr,
            rc_target_bitrate: 2000,
            rc_min_quantizer: 0,
            rc_max_quantizer: 63,
            rc_undershoot_pct: None,
            rc_overshoot_pct: None,
            rc_buf_sz: None,
            rc_buf_initial_sz: None,
            rc_buf_optimal_sz: None,
            rc_2pass_vbr_bias_pct: None,
            rc_2pass_vbr_minsection_pct: None,
            rc_2pass_vbr_maxsection_pct: None,
            fwd_kf_enabled: None,
            kf_mode: None,
            kf_min_dist: None,
            kf_max_dist: None,
            sframe_dist: None,
            sframe_mode: None,
            tile_width_count: None,
            tile_height_count: None,
            tile_widths: None,
            tile_heights: None,
            large_scale_tile: None,
            monochrome: None,
            full_still_picture_hdr: None,
            save_as_annexb: None,
            use_fixed_qp_offsets: None,

            // 制御パラメータ
            cpu_used: None,
            cq_level: None,
            sharpness: None,
            static_threshold: None,
            arnr_max_frames: None,
            arnr_strength: None,
            max_intra_bitrate_pct: None,
            lossless: None,
            row_mt: None,
            tile_columns: None,
            tile_rows: None,
            enable_tpl_model: None,
            enable_keyframe_filtering: None,
            aq_mode: None,
            deltaq_mode: None,
            noise_sensitivity: None,
            tune_content: None,
            color_primaries: None,
            transfer_characteristics: None,
            matrix_coefficients: None,
            color_range: None,
            superblock_size: None,
            enable_cdef: None,
            enable_restoration: None,
            enable_obmc: None,
            enable_global_motion: None,
            enable_warped_motion: None,
            enable_palette: None,
            enable_filter_intra: None,
            enable_smooth_intra: None,
            enable_paeth_intra: None,
            enable_cfl_intra: None,
            min_gf_interval: None,
            max_gf_interval: None,
            denoise_noise_level: None,
            denoise_block_size: None,
            film_grain_test_vector: None,
            loopfilter_control: None,
            enable_rect_partitions: None,
            enable_ab_partitions: None,
            enable_1to4_partitions: None,
            enable_dual_filter: None,
            enable_chroma_deltaq: None,
            enable_intrabc: None,
            enable_superres: None,
            gf_max_pyramid_height: None,
            max_reference_frames: None,
        }
    }
}

/// エンコード時のオプション
#[derive(Debug, Clone)]
pub struct EncodeOptions {
    /// キーフレームを強制する
    pub force_keyframe: bool,
}

// ============================================================================
// エンコーダー
// ============================================================================

/// AV1 エンコーダー
pub struct Encoder {
    ctx: sys::aom_codec_ctx,
    img: sys::aom_image,
    iter: sys::aom_codec_iter_t,
    frame_count: usize,
    image_format: ImageFormat,
    plane_sizes: PlaneSizes,
}

impl Encoder {
    /// エンコーダーインスタンスを生成する
    pub fn new(config: EncoderConfig) -> Result<Self, Error> {
        let mut cfg = MaybeUninit::<sys::aom_codec_enc_cfg>::zeroed();
        unsafe {
            let iface = sys::aom_codec_av1_cx();

            // usage パラメータでエンコーダーモードを指定する
            let usage = match config.g_usage {
                Usage::GoodQuality => sys::AOM_USAGE_GOOD_QUALITY,
                Usage::Realtime => sys::AOM_USAGE_REALTIME,
                Usage::AllIntra => sys::AOM_USAGE_ALL_INTRA,
            };
            let code = sys::aom_codec_enc_config_default(iface, cfg.as_mut_ptr(), usage);
            Error::check(code, "aom_codec_enc_config_default", None)?;

            let cfg = cfg.assume_init();
            Self::init(&config, cfg, iface)
        }
    }

    fn init(
        config: &EncoderConfig,
        mut aom_config: sys::aom_codec_enc_cfg,
        iface: *const sys::aom_codec_iface,
    ) -> Result<Self, Error> {
        // --- aom_codec_enc_cfg_t フィールドの設定 ---

        // 基本設定
        aom_config.g_w = config.g_w as _;
        aom_config.g_h = config.g_h as _;
        aom_config.g_profile = config.g_profile as _;
        aom_config.g_timebase.num = config.g_timebase.num as c_int;
        aom_config.g_timebase.den = config.g_timebase.den as c_int;

        if config.g_error_resilient {
            aom_config.g_error_resilient = 1;
        }

        if let Some(threads) = config.g_threads {
            aom_config.g_threads = threads as _;
        }

        if let Some(limit) = config.g_limit {
            aom_config.g_limit = limit as _;
        }

        if let Some(v) = config.g_forced_max_frame_width {
            aom_config.g_forced_max_frame_width = v as _;
        }

        if let Some(v) = config.g_forced_max_frame_height {
            aom_config.g_forced_max_frame_height = v as _;
        }

        if let Some(bit_depth) = config.g_bit_depth {
            aom_config.g_bit_depth = bit_depth as _;
        }

        if let Some(input_bit_depth) = config.g_input_bit_depth {
            aom_config.g_input_bit_depth = input_bit_depth as _;
        }

        if let Some(pass) = config.g_pass {
            aom_config.g_pass = match pass {
                EncodingPass::OnePass => sys::aom_enc_pass_AOM_RC_ONE_PASS,
                EncodingPass::FirstPass => sys::aom_enc_pass_AOM_RC_FIRST_PASS,
                EncodingPass::SecondPass => sys::aom_enc_pass_AOM_RC_SECOND_PASS,
                EncodingPass::ThirdPass => sys::aom_enc_pass_AOM_RC_THIRD_PASS,
            };
        }

        if let Some(lag) = config.g_lag_in_frames {
            aom_config.g_lag_in_frames = lag as _;
        }

        // レート制御
        aom_config.rc_end_usage = match config.rc_end_usage {
            RateControlMode::Vbr => sys::aom_rc_mode_AOM_VBR,
            RateControlMode::Cbr => sys::aom_rc_mode_AOM_CBR,
            RateControlMode::Cq => sys::aom_rc_mode_AOM_CQ,
            RateControlMode::Q => sys::aom_rc_mode_AOM_Q,
        };
        aom_config.rc_target_bitrate = config.rc_target_bitrate as _;
        aom_config.rc_min_quantizer = config.rc_min_quantizer as _;
        aom_config.rc_max_quantizer = config.rc_max_quantizer as _;

        if let Some(v) = config.rc_dropframe_thresh {
            aom_config.rc_dropframe_thresh = v as _;
        }
        if let Some(v) = config.rc_resize_mode {
            aom_config.rc_resize_mode = v as _;
        }
        if let Some(v) = config.rc_resize_denominator {
            aom_config.rc_resize_denominator = v as _;
        }
        if let Some(v) = config.rc_resize_kf_denominator {
            aom_config.rc_resize_kf_denominator = v as _;
        }
        if let Some(v) = config.rc_superres_mode {
            aom_config.rc_superres_mode = v as _;
        }
        if let Some(v) = config.rc_superres_denominator {
            aom_config.rc_superres_denominator = v as _;
        }
        if let Some(v) = config.rc_superres_kf_denominator {
            aom_config.rc_superres_kf_denominator = v as _;
        }
        if let Some(v) = config.rc_superres_qthresh {
            aom_config.rc_superres_qthresh = v as _;
        }
        if let Some(v) = config.rc_superres_kf_qthresh {
            aom_config.rc_superres_kf_qthresh = v as _;
        }
        if let Some(v) = config.rc_undershoot_pct {
            aom_config.rc_undershoot_pct = v as _;
        }
        if let Some(v) = config.rc_overshoot_pct {
            aom_config.rc_overshoot_pct = v as _;
        }
        if let Some(v) = config.rc_buf_sz {
            aom_config.rc_buf_sz = v as _;
        }
        if let Some(v) = config.rc_buf_initial_sz {
            aom_config.rc_buf_initial_sz = v as _;
        }
        if let Some(v) = config.rc_buf_optimal_sz {
            aom_config.rc_buf_optimal_sz = v as _;
        }
        if let Some(v) = config.rc_2pass_vbr_bias_pct {
            aom_config.rc_2pass_vbr_bias_pct = v as _;
        }
        if let Some(v) = config.rc_2pass_vbr_minsection_pct {
            aom_config.rc_2pass_vbr_minsection_pct = v as _;
        }
        if let Some(v) = config.rc_2pass_vbr_maxsection_pct {
            aom_config.rc_2pass_vbr_maxsection_pct = v as _;
        }

        // キーフレーム設定
        if let Some(enabled) = config.fwd_kf_enabled {
            aom_config.fwd_kf_enabled = if enabled { 1 } else { 0 };
        }
        if let Some(mode) = config.kf_mode {
            aom_config.kf_mode = match mode {
                KeyframeMode::Fixed => sys::aom_kf_mode_AOM_KF_FIXED,
                KeyframeMode::Auto => sys::aom_kf_mode_AOM_KF_AUTO,
            };
        }
        if let Some(v) = config.kf_min_dist {
            aom_config.kf_min_dist = v as _;
        }
        if let Some(v) = config.kf_max_dist {
            aom_config.kf_max_dist = v as _;
        }

        // S-Frame 設定
        if let Some(v) = config.sframe_dist {
            aom_config.sframe_dist = v as _;
        }
        if let Some(v) = config.sframe_mode {
            aom_config.sframe_mode = v as _;
        }

        // タイルサイズ設定
        if let Some(count) = config.tile_width_count {
            aom_config.tile_width_count = count as c_int;
        }
        if let Some(count) = config.tile_height_count {
            aom_config.tile_height_count = count as c_int;
        }
        if let Some(ref widths) = config.tile_widths {
            let len = widths.len().min(64);
            for (i, &w) in widths.iter().enumerate().take(len) {
                aom_config.tile_widths[i] = w as c_int;
            }
            aom_config.tile_width_count = len as c_int;
        }
        if let Some(ref heights) = config.tile_heights {
            let len = heights.len().min(64);
            for (i, &h) in heights.iter().enumerate().take(len) {
                aom_config.tile_heights[i] = h as c_int;
            }
            aom_config.tile_height_count = len as c_int;
        }

        // その他
        if let Some(v) = config.large_scale_tile {
            aom_config.large_scale_tile = if v { 1 } else { 0 };
        }
        if let Some(v) = config.monochrome {
            aom_config.monochrome = if v { 1 } else { 0 };
        }
        if let Some(v) = config.full_still_picture_hdr {
            aom_config.full_still_picture_hdr = if v { 1 } else { 0 };
        }
        if let Some(v) = config.save_as_annexb {
            aom_config.save_as_annexb = if v { 1 } else { 0 };
        }
        if let Some(v) = config.use_fixed_qp_offsets {
            aom_config.use_fixed_qp_offsets = if v { 1 } else { 0 };
        }

        let mut ctx = MaybeUninit::<sys::aom_codec_ctx>::zeroed();
        unsafe {
            let code = sys::aom_codec_enc_init_ver(
                ctx.as_mut_ptr(),
                iface,
                &aom_config,
                0, // flags
                sys::AOM_ENCODER_ABI_VERSION as i32,
            );
            Error::check(code, "aom_codec_enc_init_ver", None)?;

            let img_fmt = match config.image_format {
                ImageFormat::I420 => sys::aom_img_fmt_AOM_IMG_FMT_I420,
                ImageFormat::Yv12 => sys::aom_img_fmt_AOM_IMG_FMT_YV12,
                ImageFormat::Nv12 => sys::aom_img_fmt_AOM_IMG_FMT_NV12,
                ImageFormat::I422 => sys::aom_img_fmt_AOM_IMG_FMT_I422,
                ImageFormat::I444 => sys::aom_img_fmt_AOM_IMG_FMT_I444,
                ImageFormat::I42016 => sys::aom_img_fmt_AOM_IMG_FMT_I42016,
                ImageFormat::I42216 => sys::aom_img_fmt_AOM_IMG_FMT_I42216,
                ImageFormat::I44416 => sys::aom_img_fmt_AOM_IMG_FMT_I44416,
            };

            let mut img = MaybeUninit::zeroed();
            let img_ptr = sys::aom_img_alloc(
                img.as_mut_ptr(),
                img_fmt,
                aom_config.g_w,
                aom_config.g_h,
                1, // align に 1 を指定することで width == y_stride となることが保証される
            );
            if img_ptr.is_null() {
                return Err(Error::with_reason(
                    sys::aom_codec_err_t_AOM_CODEC_MEM_ERROR,
                    "aom_img_alloc",
                    "failed to allocate image buffer",
                ));
            }

            let img = img.assume_init();
            let height = config.g_h as usize;
            let plane_sizes = match config.image_format {
                ImageFormat::Nv12 => PlaneSizes::TwoPlanes {
                    y_size: height * img.stride[0] as usize,
                    uv_size: height.div_ceil(2) * img.stride[1] as usize,
                },
                // 4:2:0 系 (U/V は幅・高さともに半分)
                ImageFormat::I420 | ImageFormat::Yv12 => PlaneSizes::ThreePlanes {
                    y_size: height * img.stride[0] as usize,
                    u_size: height.div_ceil(2) * img.stride[1] as usize,
                    v_size: height.div_ceil(2) * img.stride[2] as usize,
                },
                // 4:2:2 系 (U/V は幅が半分、高さは同じ)
                ImageFormat::I422 => PlaneSizes::ThreePlanes {
                    y_size: height * img.stride[0] as usize,
                    u_size: height * img.stride[1] as usize,
                    v_size: height * img.stride[2] as usize,
                },
                // 4:4:4 系 (U/V は Y と同サイズ)
                ImageFormat::I444 => PlaneSizes::ThreePlanes {
                    y_size: height * img.stride[0] as usize,
                    u_size: height * img.stride[1] as usize,
                    v_size: height * img.stride[2] as usize,
                },
                // 16-bit 4:2:0 系
                ImageFormat::I42016 => PlaneSizes::ThreePlanes {
                    y_size: height * img.stride[0] as usize,
                    u_size: height.div_ceil(2) * img.stride[1] as usize,
                    v_size: height.div_ceil(2) * img.stride[2] as usize,
                },
                // 16-bit 4:2:2 系
                ImageFormat::I42216 => PlaneSizes::ThreePlanes {
                    y_size: height * img.stride[0] as usize,
                    u_size: height * img.stride[1] as usize,
                    v_size: height * img.stride[2] as usize,
                },
                // 16-bit 4:4:4 系
                ImageFormat::I44416 => PlaneSizes::ThreePlanes {
                    y_size: height * img.stride[0] as usize,
                    u_size: height * img.stride[1] as usize,
                    v_size: height * img.stride[2] as usize,
                },
            };

            let mut this = Self {
                ctx: ctx.assume_init(),
                img,
                iter: std::ptr::null(),
                frame_count: 0,
                image_format: config.image_format,
                plane_sizes,
            };
            // NOTE: これ以降の操作に失敗しても ctx は Drop によって確実に解放される

            // --- エンコーダー制御パラメータの設定 ---
            this.apply_controls(config)?;

            Ok(this)
        }
    }

    /// エンコーダー制御パラメータを適用する
    fn apply_controls(&mut self, config: &EncoderConfig) -> Result<(), Error> {
        // AOME_SET_CPUUSED
        if let Some(v) = config.cpu_used {
            self.set_control(sys::aome_enc_control_id_AOME_SET_CPUUSED as c_int, v)?;
        }

        // AOME_SET_CQ_LEVEL
        if let Some(v) = config.cq_level {
            self.set_control(
                sys::aome_enc_control_id_AOME_SET_CQ_LEVEL as c_int,
                v as c_int,
            )?;
        }

        // AOME_SET_SHARPNESS
        if let Some(v) = config.sharpness {
            self.set_control(
                sys::aome_enc_control_id_AOME_SET_SHARPNESS as c_int,
                v as c_int,
            )?;
        }

        // AOME_SET_STATIC_THRESHOLD
        if let Some(v) = config.static_threshold {
            self.set_control(
                sys::aome_enc_control_id_AOME_SET_STATIC_THRESHOLD as c_int,
                v as c_int,
            )?;
        }

        // AOME_SET_ARNR_MAXFRAMES
        if let Some(v) = config.arnr_max_frames {
            self.set_control(
                sys::aome_enc_control_id_AOME_SET_ARNR_MAXFRAMES as c_int,
                v as c_int,
            )?;
        }

        // AOME_SET_ARNR_STRENGTH
        if let Some(v) = config.arnr_strength {
            self.set_control(
                sys::aome_enc_control_id_AOME_SET_ARNR_STRENGTH as c_int,
                v as c_int,
            )?;
        }

        // AOME_SET_MAX_INTRA_BITRATE_PCT
        if let Some(v) = config.max_intra_bitrate_pct {
            self.set_control(
                sys::aome_enc_control_id_AOME_SET_MAX_INTRA_BITRATE_PCT as c_int,
                v as c_int,
            )?;
        }

        // AV1E_SET_LOSSLESS
        if let Some(v) = config.lossless {
            self.set_control(
                sys::aome_enc_control_id_AV1E_SET_LOSSLESS as c_int,
                if v { 1 } else { 0 },
            )?;
        }

        // AV1E_SET_ROW_MT
        if let Some(v) = config.row_mt {
            self.set_control(
                sys::aome_enc_control_id_AV1E_SET_ROW_MT as c_int,
                if v { 1 } else { 0 },
            )?;
        }

        // AV1E_SET_TILE_COLUMNS
        if let Some(v) = config.tile_columns {
            self.set_control(sys::aome_enc_control_id_AV1E_SET_TILE_COLUMNS as c_int, v)?;
        }

        // AV1E_SET_TILE_ROWS
        if let Some(v) = config.tile_rows {
            self.set_control(sys::aome_enc_control_id_AV1E_SET_TILE_ROWS as c_int, v)?;
        }

        // AV1E_SET_ENABLE_TPL_MODEL
        if let Some(v) = config.enable_tpl_model {
            self.set_control(
                sys::aome_enc_control_id_AV1E_SET_ENABLE_TPL_MODEL as c_int,
                if v { 1 } else { 0 },
            )?;
        }

        // AV1E_SET_ENABLE_KEYFRAME_FILTERING
        if let Some(v) = config.enable_keyframe_filtering {
            self.set_control(
                sys::aome_enc_control_id_AV1E_SET_ENABLE_KEYFRAME_FILTERING as c_int,
                v as c_int,
            )?;
        }

        // AV1E_SET_AQ_MODE
        if let Some(v) = config.aq_mode {
            self.set_control(
                sys::aome_enc_control_id_AV1E_SET_AQ_MODE as c_int,
                v as c_int,
            )?;
        }

        // AV1E_SET_DELTAQ_MODE
        if let Some(v) = config.deltaq_mode {
            self.set_control(
                sys::aome_enc_control_id_AV1E_SET_DELTAQ_MODE as c_int,
                v as c_int,
            )?;
        }

        // AV1E_SET_NOISE_SENSITIVITY
        if let Some(v) = config.noise_sensitivity {
            self.set_control(
                sys::aome_enc_control_id_AV1E_SET_NOISE_SENSITIVITY as c_int,
                v as c_int,
            )?;
        }

        // AV1E_SET_TUNE_CONTENT
        if let Some(tune_content) = config.tune_content {
            let content_type = match tune_content {
                ContentType::Default => sys::aom_tune_content_AOM_CONTENT_DEFAULT,
                ContentType::Screen => sys::aom_tune_content_AOM_CONTENT_SCREEN,
                ContentType::Film => sys::aom_tune_content_AOM_CONTENT_FILM,
            };
            self.set_control(
                sys::aome_enc_control_id_AV1E_SET_TUNE_CONTENT as c_int,
                content_type as c_int,
            )?;
        }

        // AV1E_SET_COLOR_PRIMARIES
        if let Some(v) = config.color_primaries {
            self.set_control(
                sys::aome_enc_control_id_AV1E_SET_COLOR_PRIMARIES as c_int,
                v as c_int,
            )?;
        }

        // AV1E_SET_TRANSFER_CHARACTERISTICS
        if let Some(v) = config.transfer_characteristics {
            self.set_control(
                sys::aome_enc_control_id_AV1E_SET_TRANSFER_CHARACTERISTICS as c_int,
                v as c_int,
            )?;
        }

        // AV1E_SET_MATRIX_COEFFICIENTS
        if let Some(v) = config.matrix_coefficients {
            self.set_control(
                sys::aome_enc_control_id_AV1E_SET_MATRIX_COEFFICIENTS as c_int,
                v as c_int,
            )?;
        }

        // AV1E_SET_COLOR_RANGE
        if let Some(v) = config.color_range {
            self.set_control(
                sys::aome_enc_control_id_AV1E_SET_COLOR_RANGE as c_int,
                v as c_int,
            )?;
        }

        // AV1E_SET_SUPERBLOCK_SIZE
        if let Some(sb_size) = config.superblock_size {
            let sb_value = match sb_size {
                SuperblockSize::Size64x64 => sys::aom_superblock_size_AOM_SUPERBLOCK_SIZE_64X64,
                SuperblockSize::Size128x128 => sys::aom_superblock_size_AOM_SUPERBLOCK_SIZE_128X128,
                SuperblockSize::Dynamic => sys::aom_superblock_size_AOM_SUPERBLOCK_SIZE_DYNAMIC,
            };
            self.set_control(
                sys::aome_enc_control_id_AV1E_SET_SUPERBLOCK_SIZE as c_int,
                sb_value as c_int,
            )?;
        }

        // AV1E_SET_ENABLE_CDEF
        if let Some(v) = config.enable_cdef {
            self.set_control(
                sys::aome_enc_control_id_AV1E_SET_ENABLE_CDEF as c_int,
                if v { 1 } else { 0 },
            )?;
        }

        // AV1E_SET_ENABLE_RESTORATION
        if let Some(v) = config.enable_restoration {
            self.set_control(
                sys::aome_enc_control_id_AV1E_SET_ENABLE_RESTORATION as c_int,
                if v { 1 } else { 0 },
            )?;
        }

        // AV1E_SET_ENABLE_OBMC
        if let Some(v) = config.enable_obmc {
            self.set_control(
                sys::aome_enc_control_id_AV1E_SET_ENABLE_OBMC as c_int,
                if v { 1 } else { 0 },
            )?;
        }

        // AV1E_SET_ENABLE_GLOBAL_MOTION
        if let Some(v) = config.enable_global_motion {
            self.set_control(
                sys::aome_enc_control_id_AV1E_SET_ENABLE_GLOBAL_MOTION as c_int,
                if v { 1 } else { 0 },
            )?;
        }

        // AV1E_SET_ENABLE_WARPED_MOTION
        if let Some(v) = config.enable_warped_motion {
            self.set_control(
                sys::aome_enc_control_id_AV1E_SET_ENABLE_WARPED_MOTION as c_int,
                if v { 1 } else { 0 },
            )?;
        }

        // AV1E_SET_ENABLE_PALETTE
        if let Some(v) = config.enable_palette {
            self.set_control(
                sys::aome_enc_control_id_AV1E_SET_ENABLE_PALETTE as c_int,
                if v { 1 } else { 0 },
            )?;
        }

        // AV1E_SET_ENABLE_FILTER_INTRA
        if let Some(v) = config.enable_filter_intra {
            self.set_control(
                sys::aome_enc_control_id_AV1E_SET_ENABLE_FILTER_INTRA as c_int,
                if v { 1 } else { 0 },
            )?;
        }

        // AV1E_SET_ENABLE_SMOOTH_INTRA
        if let Some(v) = config.enable_smooth_intra {
            self.set_control(
                sys::aome_enc_control_id_AV1E_SET_ENABLE_SMOOTH_INTRA as c_int,
                if v { 1 } else { 0 },
            )?;
        }

        // AV1E_SET_ENABLE_PAETH_INTRA
        if let Some(v) = config.enable_paeth_intra {
            self.set_control(
                sys::aome_enc_control_id_AV1E_SET_ENABLE_PAETH_INTRA as c_int,
                if v { 1 } else { 0 },
            )?;
        }

        // AV1E_SET_ENABLE_CFL_INTRA
        if let Some(v) = config.enable_cfl_intra {
            self.set_control(
                sys::aome_enc_control_id_AV1E_SET_ENABLE_CFL_INTRA as c_int,
                if v { 1 } else { 0 },
            )?;
        }

        // AV1E_SET_MIN_GF_INTERVAL
        if let Some(v) = config.min_gf_interval {
            self.set_control(
                sys::aome_enc_control_id_AV1E_SET_MIN_GF_INTERVAL as c_int,
                v as c_int,
            )?;
        }

        // AV1E_SET_MAX_GF_INTERVAL
        if let Some(v) = config.max_gf_interval {
            self.set_control(
                sys::aome_enc_control_id_AV1E_SET_MAX_GF_INTERVAL as c_int,
                v as c_int,
            )?;
        }

        // AV1E_SET_DENOISE_NOISE_LEVEL
        if let Some(v) = config.denoise_noise_level {
            self.set_control(
                sys::aome_enc_control_id_AV1E_SET_DENOISE_NOISE_LEVEL as c_int,
                v as c_int,
            )?;
        }

        // AV1E_SET_DENOISE_BLOCK_SIZE
        if let Some(v) = config.denoise_block_size {
            self.set_control(
                sys::aome_enc_control_id_AV1E_SET_DENOISE_BLOCK_SIZE as c_int,
                v as c_int,
            )?;
        }

        // AV1E_SET_FILM_GRAIN_TEST_VECTOR
        if let Some(v) = config.film_grain_test_vector {
            self.set_control(
                sys::aome_enc_control_id_AV1E_SET_FILM_GRAIN_TEST_VECTOR as c_int,
                v as c_int,
            )?;
        }

        // AV1E_SET_LOOPFILTER_CONTROL
        if let Some(v) = config.loopfilter_control {
            self.set_control(
                sys::aome_enc_control_id_AV1E_SET_LOOPFILTER_CONTROL as c_int,
                v as c_int,
            )?;
        }

        // AV1E_SET_ENABLE_RECT_PARTITIONS
        if let Some(v) = config.enable_rect_partitions {
            self.set_control(
                sys::aome_enc_control_id_AV1E_SET_ENABLE_RECT_PARTITIONS as c_int,
                if v { 1 } else { 0 },
            )?;
        }

        // AV1E_SET_ENABLE_AB_PARTITIONS
        if let Some(v) = config.enable_ab_partitions {
            self.set_control(
                sys::aome_enc_control_id_AV1E_SET_ENABLE_AB_PARTITIONS as c_int,
                if v { 1 } else { 0 },
            )?;
        }

        // AV1E_SET_ENABLE_1TO4_PARTITIONS
        if let Some(v) = config.enable_1to4_partitions {
            self.set_control(
                sys::aome_enc_control_id_AV1E_SET_ENABLE_1TO4_PARTITIONS as c_int,
                if v { 1 } else { 0 },
            )?;
        }

        // AV1E_SET_ENABLE_DUAL_FILTER
        if let Some(v) = config.enable_dual_filter {
            self.set_control(
                sys::aome_enc_control_id_AV1E_SET_ENABLE_DUAL_FILTER as c_int,
                if v { 1 } else { 0 },
            )?;
        }

        // AV1E_SET_ENABLE_CHROMA_DELTAQ
        if let Some(v) = config.enable_chroma_deltaq {
            self.set_control(
                sys::aome_enc_control_id_AV1E_SET_ENABLE_CHROMA_DELTAQ as c_int,
                if v { 1 } else { 0 },
            )?;
        }

        // AV1E_SET_ENABLE_INTRABC
        if let Some(v) = config.enable_intrabc {
            self.set_control(
                sys::aome_enc_control_id_AV1E_SET_ENABLE_INTRABC as c_int,
                if v { 1 } else { 0 },
            )?;
        }

        // AV1E_SET_ENABLE_SUPERRES
        if let Some(v) = config.enable_superres {
            self.set_control(
                sys::aome_enc_control_id_AV1E_SET_ENABLE_SUPERRES as c_int,
                if v { 1 } else { 0 },
            )?;
        }

        // AV1E_SET_GF_MAX_PYRAMID_HEIGHT
        if let Some(v) = config.gf_max_pyramid_height {
            self.set_control(
                sys::aome_enc_control_id_AV1E_SET_GF_MAX_PYRAMID_HEIGHT as c_int,
                v as c_int,
            )?;
        }

        // AV1E_SET_MAX_REFERENCE_FRAMES
        if let Some(v) = config.max_reference_frames {
            self.set_control(
                sys::aome_enc_control_id_AV1E_SET_MAX_REFERENCE_FRAMES as c_int,
                v as c_int,
            )?;
        }

        Ok(())
    }

    /// 制御パラメータを設定するヘルパー
    fn set_control(&mut self, ctrl_id: c_int, value: c_int) -> Result<(), Error> {
        let code = unsafe { sys::aom_codec_control(&mut self.ctx, ctrl_id, value) };
        Error::check(code, "aom_codec_control", Some(&self.ctx))
    }

    /// 画像データをエンコードする
    ///
    /// エンコード結果は [`Encoder::next_frame()`] で取得できる
    ///
    /// `image` のフォーマットはエンコーダー初期化時に指定した `ImageFormat` と一致する必要がある
    pub fn encode(&mut self, image: &ImageData<'_>, options: &EncodeOptions) -> Result<(), Error> {
        if !self.iter.is_null() {
            return Err(Error::with_reason(
                sys::aom_codec_err_t_AOM_CODEC_ERROR,
                "shiguredo_aom::Encoder::encode",
                "still need to call shiguredo_aom::Encoder::next_frame()",
            ));
        }

        // フォーマット整合性チェック
        if image.format() != self.image_format {
            return Err(Error::with_reason(
                sys::aom_codec_err_t_AOM_CODEC_INVALID_PARAM,
                "shiguredo_aom::Encoder::encode",
                "image format mismatch",
            ));
        }

        // プレーンサイズ検証
        match (image, &self.plane_sizes) {
            (
                ImageData::I420 { y, u, v }
                | ImageData::Yv12 { y, u, v }
                | ImageData::I422 { y, u, v }
                | ImageData::I444 { y, u, v }
                | ImageData::I42016 { y, u, v }
                | ImageData::I42216 { y, u, v }
                | ImageData::I44416 { y, u, v },
                PlaneSizes::ThreePlanes {
                    y_size,
                    u_size,
                    v_size,
                },
            ) => {
                if y.len() != *y_size || u.len() != *u_size || v.len() != *v_size {
                    return Err(Error::with_reason(
                        sys::aom_codec_err_t_AOM_CODEC_INVALID_PARAM,
                        "shiguredo_aom::Encoder::encode",
                        "invalid plane sizes",
                    ));
                }
            }
            (ImageData::Nv12 { y, uv }, PlaneSizes::TwoPlanes { y_size, uv_size }) => {
                if y.len() != *y_size || uv.len() != *uv_size {
                    return Err(Error::with_reason(
                        sys::aom_codec_err_t_AOM_CODEC_INVALID_PARAM,
                        "shiguredo_aom::Encoder::encode",
                        "invalid plane sizes",
                    ));
                }
            }
            _ => {
                return Err(Error::with_reason(
                    sys::aom_codec_err_t_AOM_CODEC_INVALID_PARAM,
                    "shiguredo_aom::Encoder::encode",
                    "invalid encoder state: image data and plane sizes mismatch",
                ));
            }
        }

        // フラグ設定
        let mut flags: sys::aom_enc_frame_flags_t = 0;
        if options.force_keyframe {
            flags |= sys::AOM_EFLAG_FORCE_KF as sys::aom_enc_frame_flags_t;
        }

        let code = unsafe {
            // 画像データをバッファにコピー
            match image {
                ImageData::I420 { y, u, v }
                | ImageData::Yv12 { y, u, v }
                | ImageData::I422 { y, u, v }
                | ImageData::I444 { y, u, v }
                | ImageData::I42016 { y, u, v }
                | ImageData::I42216 { y, u, v }
                | ImageData::I44416 { y, u, v } => {
                    std::slice::from_raw_parts_mut(self.img.planes[0], y.len()).copy_from_slice(y);
                    std::slice::from_raw_parts_mut(self.img.planes[1], u.len()).copy_from_slice(u);
                    std::slice::from_raw_parts_mut(self.img.planes[2], v.len()).copy_from_slice(v);
                }
                ImageData::Nv12 { y, uv } => {
                    std::slice::from_raw_parts_mut(self.img.planes[0], y.len()).copy_from_slice(y);
                    std::slice::from_raw_parts_mut(self.img.planes[1], uv.len())
                        .copy_from_slice(uv);
                }
            }

            // エンコード実行
            //
            // エンコーダーモード (good quality / realtime / all intra) は
            // aom_codec_enc_config_default の usage パラメータで事前に設定される。
            sys::aom_codec_encode(
                &mut self.ctx,
                &self.img,
                self.frame_count as sys::aom_codec_pts_t,
                1, // duration: 1 は「1 フレーム分」を意味する
                flags,
            )
        };
        Error::check(code, "aom_codec_encode", Some(&self.ctx))?;
        self.frame_count += 1;
        Ok(())
    }

    /// これ以上データが来ないことをエンコーダーに伝える
    ///
    /// 残りのエンコード結果は [`Encoder::next_frame()`] で取得できる
    pub fn finish(&mut self) -> Result<(), Error> {
        if !self.iter.is_null() {
            return Err(Error::with_reason(
                sys::aom_codec_err_t_AOM_CODEC_ERROR,
                "shiguredo_aom::Encoder::finish",
                "still need to call shiguredo_aom::Encoder::next_frame()",
            ));
        }

        let code = unsafe {
            sys::aom_codec_encode(
                &mut self.ctx,
                std::ptr::null(),
                -1, // pts
                0,  // duration
                0,  // flags
            )
        };
        Error::check(code, "aom_codec_encode", Some(&self.ctx))?;
        Ok(())
    }

    /// エンコード済みのフレームを取り出す
    ///
    /// [`Encoder::encode()`] や [`Encoder::finish()`] の後には、
    /// このメソッドを、結果が `None` になるまで呼び出し続ける必要がある
    pub fn next_frame(&mut self) -> Option<EncodedFrame<'_>> {
        unsafe {
            loop {
                let pkt = sys::aom_codec_get_cx_data(&mut self.ctx, &mut self.iter);
                if pkt.is_null() {
                    self.iter = std::ptr::null();
                    break;
                }

                let pkt = &*pkt;
                if pkt.kind != sys::aom_codec_cx_pkt_kind_AOM_CODEC_CX_FRAME_PKT {
                    continue;
                }

                return Some(EncodedFrame(&pkt.data.frame));
            }
        }
        None
    }
}

// 安全性: aom_codec_ctx はスレッド間で移動しても安全である。
// libaom の内部状態はスレッドローカルな資源に依存せず、
// コンテキストへの排他的アクセスがあれば（&mut self で保証される）問題ない。
unsafe impl Send for Encoder {}

impl Drop for Encoder {
    fn drop(&mut self) {
        unsafe {
            sys::aom_img_free(&mut self.img);
            sys::aom_codec_destroy(&mut self.ctx);
        }
    }
}

impl std::fmt::Debug for Encoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Encoder").finish_non_exhaustive()
    }
}

/// エンコードされた映像フレーム
pub struct EncodedFrame<'a>(&'a sys::aom_codec_cx_pkt__bindgen_ty_1__bindgen_ty_1);

impl EncodedFrame<'_> {
    /// 圧縮データ
    pub fn data(&self) -> Result<&[u8], Error> {
        let buf = self.0.buf as *const u8;
        if buf.is_null() {
            return Err(Error::with_reason(
                sys::aom_codec_err_t_AOM_CODEC_ERROR,
                "shiguredo_aom::EncodedFrame::data",
                "encoded frame buffer is null",
            ));
        }
        let sz = self.0.sz;
        if sz > isize::MAX as usize {
            return Err(Error::with_reason(
                sys::aom_codec_err_t_AOM_CODEC_ERROR,
                "shiguredo_aom::EncodedFrame::data",
                "encoded frame size exceeds isize::MAX",
            ));
        }
        Ok(unsafe { std::slice::from_raw_parts(buf, sz) })
    }

    /// キーフレームかどうか
    pub fn is_keyframe(&self) -> bool {
        (self.0.flags & sys::AOM_FRAME_IS_KEY) != 0
    }
}
