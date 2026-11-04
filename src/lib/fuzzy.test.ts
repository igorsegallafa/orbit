import { describe, expect, it } from "vitest";
import { fuzzyScore, rankPaths } from "./fuzzy";

describe("fuzzy", () => {
  it("matches subsequences and rejects the rest", () => {
    expect(fuzzyScore("src/game/game.cpp", "gamecpp")).toBeGreaterThan(0);
    expect(fuzzyScore("src/game/game.cpp", "xyz")).toBe(0);
  });

  it("ranks a matching file name above a matching folder", () => {
    const paths = ["src/game/Hud/HudWidget.cpp", "src/game/Game.cpp", "src/gameserver/Main.cpp"];
    expect(rankPaths(paths, (p) => p, "game.cpp", 3)[0]).toBe("src/game/Game.cpp");
  });
});
