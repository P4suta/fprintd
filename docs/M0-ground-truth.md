# M0 Ground Truth — 実測SLOC

`reference/` に shallow-clone した upstream（`mise run clone-ref` で再取得）を
`mise run sloc`（tokei 14.0.0）で実測する。数値は code-only（コメント・空行を除く）。

## 実測値（tokei, Code列＝コメント/空行を除く）

| サブシステム | Code | 内訳 | 備考 |
|---|---:|---|---|
| libfprint `libfprint/` 全体 | 70,913 | C 46,666 ＋ Header 23,651 | drivers＋nbis＋コア接着を再帰的に含む |
| `drivers/` | 52,685 | .c 32,130（44本）＋ .h 20,555（34本） | ヘッダ巨大＝プロトコル定数テーブル |
| `nbis/` 全体 | 8,454 | .c 7,145 ＋ .h 1,079 ＋ lua 73 | **コメント 10,130行**（≒コードより多い） |
| ┗ `nbis/mindtct` | 5,923 | .c 26本、コメント 8,757 | minutiae検出。ヒューリスティックの塊 |
| ┗ `nbis/bozorth3` | 1,222 | .c 6本 | 照合器。最小・自己完結・最も報われる |
| コア接着（= 全体−drivers−nbis） | ≈9,774 | fp-*.c / fpi-*.c / 内部ヘッダ | Rust化で大幅縮小（async/await が FpiSsm+GMainLoop を置換） |
| **fprintd（daemon本体）** | ≈5,008 | .c 4,665（9本）＋ .h 343（5本） | 残り 23,924行はPO翻訳。**置換対象ロジックは小さい** |

> 注: NBIS はコメント行が多い（.c で comment 9,357 > code 7,145）。表の値は code-only なので、
> nbis 総計は 8.5k、mindtct+bozorth3 実装分は 7.1k。

## WebP 基準での較正

WebP純Rust `image-webp` を **1ユニット**（code-only 概ね ~7–8k）とする。

- **非ドライバ算術核（MINDTCT＋BOZORTH3）= 7,145 code ≒ WebP 1ユニット**。
  1行あたりの難度は WebP より高い（ビット厳密仕様も適合ベクタも無く、検証は C 実装との出力差分のみ）。
  **BOZORTH3(1.2k) が最小で、着手の費用対効果が最も高い**。
- **コア接着 9.8k C → Rustでは数k**（`FpiSsm`＋`GMainLoop`＋`*_sync`ネストループを async/await＋state enum で置換）。
- **fprintd が再実装する daemon ロジック ≈5k C**。zbus/polkit/ファイル保存の plumbing が主。
- **ドライバ層 52.7k code（.c 32k＋.h 20k、~28 hw ドライバ）** は物理デバイス依存で有界でない。

## 含意

1. **算術核は WebP 1本分の規模**。小チームで完走可能、ハード不要でオフライン検証可能。
2. **fprintd の中身は薄い**。M1（shim-first でレイヤ検証）は速い。
3. **重いのはドライバのみ**。MOC 優先＋shim-first が正しい。
4. libfprint の `virtual-image`/`virtual-device`/`virtual-device-storage` ドライバで、
   **実機ゼロでも Docker コンテナ内で enroll/verify フローと D-Bus 契約を検証できる**。

## 計測メタ
- ツール: tokei 14.0.0（mise管理、`mise exec -- tokei`）
- 取得: `mise run clone-ref`（gitlab.freedesktop.org 本家、git cloneはAnubis非対象で成功）
- clone先: `reference/{libfprint,fprintd,libfprint-rs-binding}`（`.gitignore` 済み）
