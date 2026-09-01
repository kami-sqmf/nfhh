<script>
  // 開關。軌道用 rounded-full 而不是 rounded-pill：這個形狀不是風格選擇，
  // 半圓端點少了就不像開關了，不該跟著 token 一起壞掉（它曾經真的壞過）。
  //
  // 滑動走 translate 而不是改 left：位移交給合成器，低階手機上不會掉格。
  let { checked = false, disabled = false, label = '', onchange } = $props()
</script>

<button
  type="button"
  role="switch"
  aria-checked={checked}
  aria-label={label}
  {disabled}
  onclick={() => onchange?.(!checked)}
  class="relative w-[52px] h-8 shrink-0 p-[3px] rounded-full min-h-0
         transition-colors duration-200 ease-out
         focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-fg
         disabled:opacity-40 disabled:cursor-not-allowed
         {checked ? 'bg-ok' : 'bg-line-firm'}"
>
  <span
    class="block w-[26px] h-[26px] rounded-full bg-white
           shadow-[0_1px_3px_rgba(0,0,0,0.28)]
           transition-transform duration-200 ease-out
           {checked ? 'translate-x-[20px]' : 'translate-x-0'}"
  ></span>
</button>
