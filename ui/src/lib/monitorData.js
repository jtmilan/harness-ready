function clamp(v, min, max) {
  return Math.min(max, Math.max(min, v));
}

export function nextPoint(prev) {
  const drift = () => (Math.random() - 0.5) * 12;
  return {
    time: new Date().toLocaleTimeString("en-US", { hour12: false, minute: "2-digit", second: "2-digit" }),
    cpu: clamp((prev?.cpu ?? 45) + drift(), 8, 96),
    mem: clamp((prev?.mem ?? 60) + drift() * 0.6, 20, 92),
    net: clamp((prev?.net ?? 30) + drift(), 2, 88),
  };
}

export function initialSeries(n = 30) {
  const series = [];
  let prev = null;
  for (let i = 0; i < n; i++) {
    prev = nextPoint(prev);
    series.push(prev);
  }
  return series;
}

export function createSuccessRates(agentIds) {
  return agentIds.map((id) => ({
    id,
    success: 70 + Math.floor(Math.random() * 28),
    tasks: 12 + Math.floor(Math.random() * 60),
  }));
}

export function driftSuccessRates(rates) {
  return rates.map((r) => ({
    ...r,
    success: clamp(r.success + Math.round((Math.random() - 0.45) * 4), 55, 100),
    tasks: r.tasks + (Math.random() > 0.6 ? 1 : 0),
  }));
}