// Pointer effects: a soft light that follows the cursor across panels, and
// buttons that lean toward it. Mouse and trackpad only, and nothing at all
// for readers who ask for reduced motion.
(function () {
  if (!matchMedia('(hover: hover) and (pointer: fine)').matches) return;
  if (matchMedia('(prefers-reduced-motion: reduce)').matches) return;

  var SPOT = '.cell:not(footer .cell), .tip';
  var PULL = 0.18; // share of the distance a button travels toward the cursor
  var MAX = 6; // and never more than this, in pixels

  var frame = 0;
  var last;

  function update() {
    frame = 0;
    var e = last;

    var spot = e.target.closest && e.target.closest(SPOT);
    if (spot) {
      var r = spot.getBoundingClientRect();
      spot.style.setProperty('--mx', (e.clientX - r.left) + 'px');
      spot.style.setProperty('--my', (e.clientY - r.top) + 'px');
    }

    var pill = e.target.closest && e.target.closest('.pill');
    if (pill) {
      var p = pill.getBoundingClientRect();
      var dx = (e.clientX - (p.left + p.width / 2)) * PULL;
      var dy = (e.clientY - (p.top + p.height / 2)) * PULL;
      dx = Math.max(-MAX, Math.min(MAX, dx));
      dy = Math.max(-MAX, Math.min(MAX, dy));
      pill.style.transform = 'translate(' + dx + 'px,' + dy + 'px)';
    }
  }

  document.addEventListener('pointermove', function (e) {
    last = e;
    if (!frame) frame = requestAnimationFrame(update);
  }, { passive: true });

  // Buttons settle back when the cursor leaves them.
  document.querySelectorAll('.pill').forEach(function (pill) {
    pill.addEventListener('pointerleave', function () { pill.style.transform = ''; });
  });
})();

// The footer wordmark, after the logo: a moon over still glass. The word
// stands on its own reflection; with a mouse, a moon follows the cursor,
// the letters under it rise into its light, and moving disturbs the glass.
(function () {
  var giant = document.querySelector('.giant');
  if (!giant) return;

  var word = giant.textContent.trim();
  giant.textContent = '';
  function line(cls) {
    var s = document.createElement('span');
    s.className = 'gw ' + cls;
    Array.from(word).forEach(function (ch) {
      var l = document.createElement('span');
      l.textContent = ch;
      s.appendChild(l);
    });
    return s;
  }
  var stage = document.createElement('span');
  stage.className = 'gw-stage';
  var moon = document.createElement('span');
  moon.className = 'moon';
  var moonR = document.createElement('span');
  moonR.className = 'moon moon-r';
  var front = line('gw-front');
  var back = line('gw-reflect');
  stage.append(moon, moonR, front, back);
  giant.appendChild(stage);
  giant.classList.add('gw-ready');

  if (!matchMedia('(hover: hover) and (pointer: fine)').matches) return;
  if (matchMedia('(prefers-reduced-motion: reduce)').matches) return;

  // The ripple: noise that pushes the reflection's pixels around, as
  // strong as the cursor has lately been moving.
  var NS = 'http://www.w3.org/2000/svg';
  var svg = document.createElementNS(NS, 'svg');
  svg.setAttribute('width', '0');
  svg.setAttribute('height', '0');
  svg.setAttribute('aria-hidden', 'true');
  svg.style.position = 'absolute';
  svg.innerHTML =
    '<filter id="gw-ripple" x="-5%" y="-30%" width="110%" height="160%">' +
    '<feTurbulence type="fractalNoise" baseFrequency="0.006 0.09" numOctaves="2" seed="7" result="n"/>' +
    '<feDisplacementMap in="SourceGraphic" in2="n" scale="0" xChannelSelector="R" yChannelSelector="G"/>' +
    '</filter>';
  document.body.appendChild(svg);
  var turb = svg.querySelector('feTurbulence');
  var disp = svg.querySelector('feDisplacementMap');
  back.style.filter = 'url(#gw-ripple)';

  var fronts = Array.from(front.children);
  var backs = Array.from(back.children);
  var glow = fronts.map(function () { return 0; });
  var lit = 0; // how far the word has dimmed around the moon
  var ripple = 0;
  var phase = 0;
  var inside = false;
  var px = 0, py = 0, lastX = null, lastY = null;
  var running = false;

  giant.addEventListener('pointermove', function (e) {
    var r = stage.getBoundingClientRect();
    px = e.clientX - r.left;
    py = e.clientY - r.top;
    if (lastX !== null) {
      ripple = Math.min(22, ripple + Math.hypot(e.clientX - lastX, e.clientY - lastY) * 0.12);
    }
    lastX = e.clientX;
    lastY = e.clientY;
    inside = true;
    giant.classList.add('lit');
    start();
  });
  giant.addEventListener('pointerleave', function () {
    inside = false;
    lastX = lastY = null;
    giant.classList.remove('lit');
  });

  function start() {
    if (!running) {
      running = true;
      requestAnimationFrame(step);
    }
  }

  function step() {
    var em = parseFloat(getComputedStyle(giant).fontSize);
    var reach = em * 1.1; // how far the moonlight spreads, sideways
    var axis = front.offsetHeight - em * 0.2; // the waterline
    var busy = false;

    lit += ((inside ? 1 : 0) - lit) * 0.08;
    fronts.forEach(function (l, i) {
      var dx = (l.offsetLeft + l.offsetWidth / 2 - px) / reach;
      var target = inside ? Math.exp(-dx * dx) : 0;
      glow[i] += (target - glow[i]) * 0.14;
      var g = glow[i];
      if (Math.abs(target - g) > 0.002) busy = true;
      var css = 'translateY(' + (-g * 0.09).toFixed(4) + 'em)';
      var op = (1 - lit * 0.5 * (1 - g)).toFixed(3);
      var shadow = '0 0 ' + (g * 0.35).toFixed(3) + 'em rgba(255,255,255,' + (g * 0.55).toFixed(3) + ')';
      // The reflection is flipped, so the same lift moves it down.
      [l, backs[i]].forEach(function (el) {
        el.style.transform = css;
        el.style.opacity = op;
        el.style.textShadow = shadow;
      });
    });

    var size = moon.offsetWidth;
    moon.style.transform = 'translate(' + (px - size / 2) + 'px,' + (py - size / 2) + 'px)';
    moonR.style.transform = 'translate(' + (px - size / 2) + 'px,' + (2 * axis - py - size / 2) + 'px)';

    ripple *= 0.94;
    phase += 0.03;
    turb.setAttribute('baseFrequency', '0.006 ' + (0.09 + 0.012 * Math.sin(phase)).toFixed(4));
    disp.setAttribute('scale', ripple.toFixed(2));

    if (inside || busy || ripple > 0.05 || Math.abs(lit) > 0.002) {
      requestAnimationFrame(step);
    } else {
      disp.setAttribute('scale', '0');
      running = false;
    }
  }
})();
