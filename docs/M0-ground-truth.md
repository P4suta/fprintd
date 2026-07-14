# M0 Ground Truth — 実測SLOCと見積り確定

`mise run sloc`（tokei 14.0.0）で upstream を実測した結果。research フェーズの
ghloc 推定（blanks+comments込み）を **code-only** で置き換える。計測対象は
`reference/` に shallow-clone した upstream（`mise run clone-ref` で再取得可能）。

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

> 注: ghloc（research）は libfprint core を ~63k、nbis を ~18.4k と報告したが、これは
> blanks+comments込み。NBIS はコメント率が高く（.c で 9,357 comment vs 7,145 code）、
> **code-only では nbis 総計 8.5k / mindtct+bozorth3 実装分 7.1k** に縮む。

## WebP 基準での再較正

WebP純Rust `image-webp` を **1ユニット**（ghloc 9.3k / code-only 概ね ~7–8k）とする。

- **非ドライバ算術核（MINDTCT＋BOZORTH3）= 7,145 code ≒ WebP 1ユニット**。
  research統合の「2〜4ユニット」は上記コメント膨張による過大評価だった。
  ただし *1行あたりの難度* は WebP より高い（RFCのようなビット厳密仕様も適合ベクタも無く、
  検証は C 実装との出力差分のみ）。**BOZORTH3(1.2k)から着手が最も費用対効果が高い**。
- **コア接着 9.8k C → Rustでは数k**（`FpiSsm`＋`GMainLoop`＋`*_sync`ネストループを async/await＋state enum で置換）。
- **fprintd-rs が再実装する daemon ロジック ≈5k C** → 小さい。zbus/polkit/ファイル保存の plumbing が主。
- **ドライバ層 52.7k code（.c 32k＋.h 20k、~28 hw ドライバ）** → 有界でなく、物理デバイス依存。見積り不能軸のまま。

## 確定した含意

1. **算術核はWebP1本分規模**＝小チームで完走可能・ハード不要でオフライン検証可能。当初想定より軽い。
2. **fprintd-rs の中身は薄い**＝M1（shim-first で世界向けレイヤ検証）は現実的に速い。
3. **重いのはドライバだけ**という結論は不変。だから MOC 優先＋shim-first が正しい。
4. libfprint には `virtual-image`/`virtual-device`/`virtual-device-storage` ドライバがある
   → **実機ゼロでも Docker コンテナ内で enroll/verify フローと D-Bus 契約を検証できる**（M1加速）。

## 計測メタ
- ツール: tokei 14.0.0（mise管理、`mise exec -- tokei`）
- 取得: `mise run clone-ref`（gitlab.freedesktop.org 本家、git cloneはAnubis非対象で成功）
- clone先: `reference/{libfprint,fprintd,libfprint-rs-binding}`（`.gitignore` 済み）
