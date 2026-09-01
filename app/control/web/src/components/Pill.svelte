<script>
  // 1a 的狀態語彙：綠＝確定成立，琥珀＝還在觀察，紅＝被拒絕或撤銷，
  // 灰＝無資訊。灰永不與紅混用 —— 「不知道」不是「不通過」。
  let { tone = 'none', children } = $props()

  const tones = {
    ok: 'bg-ok-bg text-ok-fg',
    watch: 'bg-watch-bg text-watch-fg',
    bad: 'bg-bad-bg text-bad-fg',
    none: 'bg-wash text-fg-faint',
  }
</script>

<span class="font-mono text-label font-medium px-3 py-1.5 rounded-pill whitespace-nowrap {tones[tone]}">
  {@render children()}
</span>
