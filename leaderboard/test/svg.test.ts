import { describe, expect, it } from "vitest";
import { conditionName, renderLeaderboardSvg } from "../src/svg";

describe("leaderboard SVG condition labels", () => {
  it.each([
    [0, "Standard"],
    [1, "Rush Hour"],
    [2, "Chicken Frenzy"],
    [3, "Stampede"],
    [4, "Glass Cannon"],
  ])("maps condition ID %i to %s", (id, name) => {
    expect(conditionName(id)).toBe(name);
  });

  it("uses an explicit fallback for an unknown stored condition ID", () => {
    expect(conditionName(9)).toBe("Unknown (ID 9)");
  });

  it("renders player-facing condition names instead of Road/C-number labels", () => {
    const svg = renderLeaderboardSvg(
      [{ rank: 1, name: "MODZ", score: 1614, condition: 2 }],
      null,
      0,
    );
    expect(svg).toContain(">CONDITION</text>");
    expect(svg).toContain(">Chicken Frenzy</text>");
    expect(svg).not.toContain(">ROAD</text>");
    expect(svg).not.toContain(">C2</text>");
  });

  it("names a condition-filtered board with the selected condition", () => {
    const svg = renderLeaderboardSvg([], 2, 0);
    expect(svg).toContain("Chicken Frenzy leaderboard");
    expect(svg).toContain(">CHICKEN FRENZY LEADERBOARD</text>");
  });
});
