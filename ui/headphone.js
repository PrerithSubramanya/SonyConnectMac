// Original vector drawing of an over-ear headphone in the WH-1000XM5 style.
// Hand-drawn for this app. It is not a Sony product photo or logo.
//
// Colours come from CSS variables so one drawing serves all four colourways.
window.headphoneSVG = function headphoneSVG() {
  return `
<svg class="hp" viewBox="0 0 400 330" aria-hidden="true">
  <defs>
    <linearGradient id="gBand" x1="0" y1="0" x2="0" y2="1">
      <stop offset="0" style="stop-color:var(--hp-hi)"/>
      <stop offset="1" style="stop-color:var(--hp-lo)"/>
    </linearGradient>
    <linearGradient id="gCup" x1="0.15" y1="0" x2="0.85" y2="1">
      <stop offset="0" style="stop-color:var(--hp-cup-hi)"/>
      <stop offset="1" style="stop-color:var(--hp-cup-lo)"/>
    </linearGradient>
    <radialGradient id="gGloss" cx="30%" cy="22%" r="75%">
      <stop offset="0" stop-color="#fff" stop-opacity=".42"/>
      <stop offset="1" stop-color="#fff" stop-opacity="0"/>
    </radialGradient>
    <radialGradient id="gLed" cx="50%" cy="50%" r="50%">
      <stop offset="0" stop-color="#8fd0ff"/>
      <stop offset="1" stop-color="#0a84ff"/>
    </radialGradient>
  </defs>

  <g class="hp-float">
    <g class="band-g">
      <path class="band" pathLength="1" d="M96 210 C 88 84 142 34 200 34 C 258 34 312 84 304 210"/>
      <path class="band-shine" pathLength="1" d="M110 188 C 108 98 150 50 200 50 C 250 50 292 98 290 188"/>
      <path class="band-pad" pathLength="1" d="M124 190 C 122 114 158 76 200 76 C 242 76 278 114 276 190"/>
    </g>

    <g class="cup-anim cup-anim-l">
      <g transform="rotate(-4 96 252)">
        <ellipse class="pad" cx="130" cy="256" rx="20" ry="58"/>
        <ellipse cx="96" cy="252" rx="48" ry="72" fill="url(#gCup)"/>
        <ellipse cx="96" cy="252" rx="48" ry="72" fill="url(#gGloss)"/>
        <ellipse class="cup-ring" cx="96" cy="252" rx="37" ry="60"/>
        <circle class="led" cx="96" cy="200" r="4" fill="url(#gLed)"/>
        <circle class="mic" cx="96" cy="306" r="2.2"/>
      </g>
      <g class="waves waves-l">
        <path d="M44 232 q-12 20 0 40"/><path d="M32 220 q-20 32 0 64"/>
      </g>
    </g>

    <g class="cup-anim cup-anim-r">
      <g transform="rotate(4 304 252)">
        <ellipse class="pad" cx="270" cy="256" rx="20" ry="58"/>
        <ellipse cx="304" cy="252" rx="48" ry="72" fill="url(#gCup)"/>
        <ellipse cx="304" cy="252" rx="48" ry="72" fill="url(#gGloss)"/>
        <ellipse class="cup-ring" cx="304" cy="252" rx="37" ry="60"/>
        <circle class="mic" cx="304" cy="200" r="2.2"/>
        <circle class="mic" cx="304" cy="306" r="2.2"/>
      </g>
      <g class="waves waves-r">
        <path d="M356 232 q12 20 0 40"/><path d="M368 220 q20 32 0 64"/>
      </g>
    </g>
  </g>
</svg>`;
};

// Colour names follow the headphone's own colour code (0x01 black, 0x03 silver, ...).
window.HP_COLORS = {
  black:  { hi: '#454851', lo: '#1c1d22', cupHi: '#4d515b', cupLo: '#1b1c20' },
  silver: { hi: '#f4f2ec', lo: '#c7c2b9', cupHi: '#faf8f3', cupLo: '#cdc8bf' },
  blue:   { hi: '#51657f', lo: '#202b3c', cupHi: '#5a6f8b', cupLo: '#1f2a3a' },
  pink:   { hi: '#e2c3bf', lo: '#a98480', cupHi: '#ecd0cc', cupLo: '#b08c88' },
};

window.applyHeadphoneColor = function applyHeadphoneColor(el, key) {
  const c = window.HP_COLORS[key] || window.HP_COLORS.black;
  el.style.setProperty('--hp-hi', c.hi);
  el.style.setProperty('--hp-lo', c.lo);
  el.style.setProperty('--hp-cup-hi', c.cupHi);
  el.style.setProperty('--hp-cup-lo', c.cupLo);
};
