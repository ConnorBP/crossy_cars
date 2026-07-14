// Accessible SVG rendering for the public Roady Car leaderboard badge.

export interface SvgLeaderboardEntry {
  rank: number;
  name: string;
  score: number;
  condition: number;
}

const CONDITION_NAMES = [
  "Standard",
  "Rush Hour",
  "Chicken Frenzy",
  "Stampede",
  "Glass Cannon",
] as const;

/** Player-facing name for a stable stored condition ID. */
export function conditionName(condition: number): string {
  return CONDITION_NAMES[condition] ?? `Unknown (ID ${condition})`;
}

/** Escape untrusted text before interpolating it into XML text or attributes. */
export function escapeXml(value: unknown): string {
  return String(value).replace(/[&<>"']/g, (character) => {
    switch (character) {
      case "&": return "&amp;";
      case "<": return "&lt;";
      case ">": return "&gt;";
      case '"': return "&quot;";
      case "'": return "&apos;";
      default: return character;
    }
  });
}

/** Render a fixed-width, variable-height dark/gold arcade leaderboard SVG. */
export function renderLeaderboardSvg(
  entries: readonly SvgLeaderboardEntry[],
  condition: number | null,
  generatedAt: number,
): string {
  const width = 720;
  const rowHeight = 42;
  const height = entries.length === 0 ? 300 : 230 + entries.length * rowHeight;
  const boardName = condition === null ? "Global" : conditionName(condition);
  const boardLabel = boardName.toUpperCase();
  const title = `Roady Car ${boardName} leaderboard`;
  const description = entries.length === 0
    ? "No live scores yet."
    : `Top ${entries.length} live score${entries.length === 1 ? "" : "s"}, ordered by score.`;
  const generated = new Date(generatedAt).toISOString();

  const rows = entries.map((entry, index) => {
    const y = 178 + index * rowHeight;
    const stripe = index % 2 === 0
      ? `<rect x="24" y="${y - 27}" width="672" height="38" rx="4" fill="#18170f"/>`
      : "";
    return `${stripe}
      <text x="48" y="${y}" class="rank">${escapeXml(entry.rank.toString().padStart(2, "0"))}</text>
      <text x="128" y="${y}" class="name">${escapeXml(entry.name)}</text>
      <text x="520" y="${y}" class="score" text-anchor="end">${escapeXml(entry.score)}</text>
      <text x="684" y="${y}" class="condition" text-anchor="end">${escapeXml(conditionName(entry.condition))}</text>`;
  }).join("\n");

  const content = entries.length === 0
    ? `<text x="360" y="190" class="empty" text-anchor="middle">NO LIVE SCORES YET</text>
      <text x="360" y="220" class="emptyHint" text-anchor="middle">BE THE FIRST TO HIT THE ROAD</text>`
    : rows;
  const timestampY = height - 28;

  return `<svg xmlns="http://www.w3.org/2000/svg" width="${width}" height="${height}" viewBox="0 0 ${width} ${height}" role="img" aria-labelledby="leaderboard-title leaderboard-description">
  <title id="leaderboard-title">${escapeXml(title)}</title>
  <desc id="leaderboard-description">${escapeXml(description)}</desc>
  <style>
    text { font-family: ui-monospace, "Courier New", monospace; }
    .kicker { fill: #d7a928; font-size: 14px; font-weight: 700; letter-spacing: 4px; }
    .heading { fill: #fff3bd; font-size: 30px; font-weight: 900; letter-spacing: 2px; }
    .columns { fill: #a58d4d; font-size: 12px; font-weight: 700; letter-spacing: 2px; }
    .rank { fill: #d7a928; font-size: 18px; font-weight: 800; }
    .condition { fill: #d7a928; font-size: 14px; font-weight: 800; }
    .name { fill: #fff3bd; font-size: 19px; font-weight: 800; letter-spacing: 2px; }
    .score { fill: #ffffff; font-size: 19px; font-weight: 900; }
    .empty { fill: #fff3bd; font-size: 19px; font-weight: 800; letter-spacing: 2px; }
    .emptyHint, .timestamp { fill: #81764f; font-size: 11px; letter-spacing: 1px; }
  </style>
  <rect width="${width}" height="${height}" rx="12" fill="#0b0b09"/>
  <rect x="8" y="8" width="704" height="${height - 16}" rx="8" fill="none" stroke="#d7a928" stroke-width="2"/>
  <path d="M24 104 H696" stroke="#4e411b" stroke-width="2"/>
  <text x="360" y="45" class="kicker" text-anchor="middle">ROADY CAR</text>
  <text x="360" y="82" class="heading" text-anchor="middle">${escapeXml(boardLabel)} LEADERBOARD</text>
  <text x="48" y="132" class="columns">RANK</text>
  <text x="128" y="132" class="columns">DRIVER</text>
  <text x="520" y="132" class="columns" text-anchor="end">SCORE</text>
  <text x="684" y="132" class="columns" text-anchor="end">CONDITION</text>
  ${content}
  <text x="360" y="${timestampY}" class="timestamp" text-anchor="middle">GENERATED ${escapeXml(generated)}</text>
</svg>`;
}
