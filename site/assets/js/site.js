/* Sceadu — small, dependency-free site runtime. */
(() => {
  "use strict";

  const reduceMotion = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  const $ = (selector, root = document) => root.querySelector(selector);
  const $$ = (selector, root = document) => Array.from(root.querySelectorAll(selector));

  /* ---------------------------------------------------------------------
     Navigation
     --------------------------------------------------------------------- */

  const nav = $("#site-nav");
  const menuToggle = $("#menu-toggle");
  const mobileMenu = $("#mobile-menu");

  const onScroll = () => {
    const y = window.scrollY;
    if (!nav) return;
    const shouldPin = y > 8;
    if (shouldPin !== nav.classList.contains("is-scrolled")) {
      nav.classList.toggle("is-scrolled", shouldPin);
    }
  };

  if (nav) {
    onScroll();
    window.addEventListener("scroll", onScroll, { passive: true });
  }

  if (menuToggle && mobileMenu) {
    const setMenu = (open) => {
      menuToggle.setAttribute("aria-expanded", String(open));
      menuToggle.setAttribute("aria-label", open ? "Close menu" : "Open menu");
      mobileMenu.hidden = !open;
      document.body.classList.toggle("menu-open", open);
    };

    menuToggle.addEventListener("click", () => {
      const open = menuToggle.getAttribute("aria-expanded") !== "true";
      setMenu(open);
      if (open) {
        const first = mobileMenu.querySelector("a");
        if (first) first.focus({ preventScroll: true });
      }
    });

    $$("a", mobileMenu).forEach((link) => {
      link.addEventListener("click", () => setMenu(false));
    });

    document.addEventListener("keydown", (event) => {
      if (event.key === "Escape" && !mobileMenu.hidden) {
        setMenu(false);
        menuToggle.focus();
      }
    });

    const desktop = window.matchMedia("(min-width: 901px)");
    desktop.addEventListener("change", (event) => {
      if (event.matches) setMenu(false);
    });
  }

  /* ---------------------------------------------------------------------
     Copy to clipboard
     --------------------------------------------------------------------- */

  const live = document.createElement("div");
  live.setAttribute("aria-live", "polite");
  live.className = "sr-only";
  document.body.appendChild(live);

  const copyText = async (text) => {
    if (navigator.clipboard && window.isSecureContext) {
      try {
        await navigator.clipboard.writeText(text);
        return true;
      } catch {
        // Permission denied or unavailable: fall through to the textarea path.
      }
    }
    const area = document.createElement("textarea");
    area.value = text;
    area.setAttribute("readonly", "");
    area.style.position = "fixed";
    area.style.top = "-1000px";
    area.style.opacity = "0";
    document.body.appendChild(area);
    area.select();
    let ok = false;
    try {
      ok = document.execCommand("copy");
    } catch {
      ok = false;
    }
    area.remove();
    return ok;
  };

  $$("[data-copy]").forEach((button) => {
    button.addEventListener("click", async () => {
      const target = document.getElementById(button.dataset.copy);
      if (!target) return;
      const text = target.textContent.replace(/\n{3,}/g, "\n\n").trim();
      const label = button.querySelector("span");
      let ok = false;
      try {
        ok = await copyText(text);
      } catch {
        ok = false;
      }
      if (!ok) {
        const range = document.createRange();
        range.selectNodeContents(target);
        const selection = window.getSelection();
        if (selection) {
          selection.removeAllRanges();
          selection.addRange(range);
        }
        live.textContent = "Copy failed. Text selected; press Control-C or Command-C.";
        if (label) label.textContent = "Selected";
        window.setTimeout(() => {
          if (label) label.textContent = "Copy";
        }, 2200);
        return;
      }
      button.classList.add("is-copied");
      if (label) label.textContent = "Copied";
      live.textContent = "Copied to clipboard.";
      window.setTimeout(() => {
        button.classList.remove("is-copied");
        if (label) label.textContent = "Copy";
      }, 1800);
    });
  });

  /* ---------------------------------------------------------------------
     Scroll reveal
     --------------------------------------------------------------------- */

  const reveals = $$(".reveal");
  if (reduceMotion) {
    reveals.forEach((el) => el.classList.add("in"));
  } else {
    const pending = new Set(reveals);
    let revealFrame = 0;

    const checkReveal = () => {
      revealFrame = 0;
      const trigger = window.innerHeight * 0.92;
      for (const el of pending) {
        if (el.getBoundingClientRect().top < trigger) {
          el.classList.add("in");
          pending.delete(el);
        }
      }
      if (!pending.size) {
        window.removeEventListener("scroll", queueReveal);
        window.removeEventListener("resize", queueReveal);
      }
    };

    const queueReveal = () => {
      if (revealFrame) return;
      revealFrame = window.requestAnimationFrame(checkReveal);
    };

    window.addEventListener("scroll", queueReveal, { passive: true });
    window.addEventListener("resize", queueReveal);
    checkReveal();
  }

  /* ---------------------------------------------------------------------
     Demo video
     --------------------------------------------------------------------- */

  const video = $("#demo-video");
  const toggle = $("#demo-toggle");
  const overlay = $("#demo-overlay");

  if (video && toggle) {
    const frame = video.closest(".demo-frame");
    let started = false;

    const sync = () => {
      const paused = video.paused;
      toggle.textContent = paused ? "Play" : "Pause";
      if (frame) frame.classList.toggle("is-playing", !paused);
      if (overlay) overlay.hidden = !paused || started;
    };

    const play = () => {
      const attempt = video.play();
      if (attempt && typeof attempt.catch === "function") {
        attempt.catch(() => sync());
      }
    };

    toggle.addEventListener("click", () => {
      if (video.paused) play();
      else video.pause();
    });

    if (overlay) {
      overlay.addEventListener("click", play);
    }

    video.addEventListener("click", () => {
      if (video.paused) play();
      else video.pause();
    });

    video.addEventListener("play", () => {
      started = true;
      sync();
    });
    video.addEventListener("pause", sync);
    sync();

    if (!reduceMotion) {
      if ("IntersectionObserver" in window && video.closest(".demo-frame")) {
        const demoObserver = new IntersectionObserver(
          (entries) => {
            if (entries.some((entry) => entry.isIntersecting)) {
              play();
              demoObserver.disconnect();
            }
          },
          { threshold: 0.35 }
        );
        demoObserver.observe(video);
      } else {
        play();
      }
    }
  }

  /* ---------------------------------------------------------------------
     Hero mesh — ambient topology, no external library
     --------------------------------------------------------------------- */

  const canvas = $("#mesh");
  if (!canvas) return;

  const ctx = canvas.getContext("2d");
  if (!ctx) return;

  const hero = canvas.closest(".hero");
  let width = 0;
  let height = 0;
  let dpr = 1;
  let nodes = [];
  let packets = [];
  let raf = 0;
  let running = true;
  let inView = true;
  let last = performance.now();
  let mouse = { x: -1000, y: -1000, active: false };
  let packetClock = 0;

  const rand = (min, max) => min + Math.random() * (max - min);

  const makeNodes = () => {
    const target = Math.max(26, Math.min(78, Math.round((width * height) / 25000)));
    nodes = Array.from({ length: target }, () => ({
      x: rand(0, width),
      y: rand(0, height),
      vx: rand(-0.055, 0.055),
      vy: rand(-0.055, 0.055),
      r: rand(0.7, 1.7),
      depth: rand(0.3, 1)
    }));
    packets = [];
  };

  const resize = () => {
    const rect = canvas.getBoundingClientRect();
    dpr = Math.min(window.devicePixelRatio || 1, 2);
    width = Math.max(1, rect.width);
    height = Math.max(1, rect.height);
    canvas.width = Math.round(width * dpr);
    canvas.height = Math.round(height * dpr);
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    makeNodes();
    if (reduceMotion) draw(0, true);
  };

  const spawnPacket = () => {
    if (nodes.length < 2) return;
    const from = nodes[Math.floor(Math.random() * nodes.length)];
    let to = null;
    let best = Infinity;
    for (const node of nodes) {
      if (node === from) continue;
      const d = Math.hypot(node.x - from.x, node.y - from.y);
      if (d < 170 && Math.random() < 0.5 && d < best) {
        best = d;
        to = node;
      }
    }
    if (!to) return;
    packets.push({ from, to, t: 0, speed: rand(0.005, 0.011) });
  };

  const draw = (delta, still = false) => {
    ctx.clearRect(0, 0, width, height);

    const linkDist = Math.min(150, Math.max(95, width * 0.1));
    ctx.lineWidth = 0.7;

    for (let i = 0; i < nodes.length; i += 1) {
      const a = nodes[i];
      for (let j = i + 1; j < nodes.length; j += 1) {
        const b = nodes[j];
        const dx = a.x - b.x;
        const dy = a.y - b.y;
        const dist = Math.hypot(dx, dy);
        if (dist > linkDist) continue;
        const falloff = 1 - dist / linkDist;
        const nearMouse =
          mouse.active &&
          (Math.hypot(a.x - mouse.x, a.y - mouse.y) < 160 ||
            Math.hypot(b.x - mouse.x, b.y - mouse.y) < 160);
        ctx.strokeStyle = `rgba(215, 255, 79, ${(falloff * 0.17 + (nearMouse ? 0.1 : 0)).toFixed(3)})`;
        ctx.beginPath();
        ctx.moveTo(a.x, a.y);
        ctx.lineTo(b.x, b.y);
        ctx.stroke();
      }
    }

    if (mouse.active) {
      for (const node of nodes) {
        const dist = Math.hypot(node.x - mouse.x, node.y - mouse.y);
        if (dist > 175) continue;
        ctx.strokeStyle = `rgba(233, 236, 230, ${((1 - dist / 175) * 0.16).toFixed(3)})`;
        ctx.beginPath();
        ctx.moveTo(node.x, node.y);
        ctx.lineTo(mouse.x, mouse.y);
        ctx.stroke();
      }
    }

    for (const node of nodes) {
      const nearMouse =
        mouse.active && Math.hypot(node.x - mouse.x, node.y - mouse.y) < 150;
      const alpha = 0.3 + node.depth * 0.24 + (nearMouse ? 0.25 : 0);
      ctx.fillStyle = `rgba(233, 236, 230, ${alpha.toFixed(3)})`;
      ctx.beginPath();
      ctx.arc(node.x, node.y, node.r, 0, Math.PI * 2);
      ctx.fill();
    }

    for (const packet of packets) {
      const x = packet.from.x + (packet.to.x - packet.from.x) * packet.t;
      const y = packet.from.y + (packet.to.y - packet.from.y) * packet.t;
      const glow = ctx.createRadialGradient(x, y, 0, x, y, 9);
      glow.addColorStop(0, "rgba(215, 255, 79, 0.85)");
      glow.addColorStop(1, "rgba(215, 255, 79, 0)");
      ctx.fillStyle = glow;
      ctx.beginPath();
      ctx.arc(x, y, 9, 0, Math.PI * 2);
      ctx.fill();
      ctx.fillStyle = "rgba(215, 255, 79, 0.95)";
      ctx.beginPath();
      ctx.arc(x, y, 1.4, 0, Math.PI * 2);
      ctx.fill();
    }

    if (still) return;

    const step = Math.min(delta, 50) / 16.67;
    for (const node of nodes) {
      node.x += node.vx * step;
      node.y += node.vy * step;
      if (node.x < -20) node.x = width + 20;
      if (node.x > width + 20) node.x = -20;
      if (node.y < -20) node.y = height + 20;
      if (node.y > height + 20) node.y = -20;
    }

    packetClock += delta;
    if (packetClock > 1300 && packets.length < 7) {
      packetClock = 0;
      spawnPacket();
    }

    for (const packet of packets) packet.t += packet.speed * step;
    packets = packets.filter((packet) => packet.t < 1);
  };

  const tick = (now) => {
    const delta = now - last;
    last = now;
    if (running && inView) draw(delta);
    raf = window.requestAnimationFrame(tick);
  };

  const start = () => {
    if (raf) return;
    last = performance.now();
    raf = window.requestAnimationFrame(tick);
  };

  const stop = () => {
    if (!raf) return;
    window.cancelAnimationFrame(raf);
    raf = 0;
  };

  resize();

  if (reduceMotion) {
    draw(0, true);
  } else {
    start();

    const resizeObserver = new ResizeObserver(() => resize());
    resizeObserver.observe(canvas);

    if (hero && "IntersectionObserver" in window) {
      const visibility = new IntersectionObserver(
        (entries) => {
          inView = entries[0] ? entries[0].isIntersecting : true;
        },
        { threshold: 0.02 }
      );
      visibility.observe(hero);
    }

    document.addEventListener("visibilitychange", () => {
      running = !document.hidden;
      if (running) last = performance.now();
    });

    const pointerSurface = hero || canvas;

    pointerSurface.addEventListener("pointermove", (event) => {
      const rect = canvas.getBoundingClientRect();
      mouse = {
        x: event.clientX - rect.left,
        y: event.clientY - rect.top,
        active: true
      };
    });

    pointerSurface.addEventListener("pointerleave", () => {
      mouse.active = false;
    });
    window.addEventListener("blur", () => {
      mouse.active = false;
    });
  }

  window.addEventListener(
    "pagehide",
    () => {
      stop();
    },
    { once: true }
  );
})();
