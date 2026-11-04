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
  it("ranks an exact file name above longer ones", () => {
    const paths = ["proto/gameserver.proto", "src/Engine/GameLoop.cpp", "src/game/Game.cpp", "src/game/Game.h"];
    expect(rankPaths(paths, (p) => p, "Game", 4).slice(0, 2)).toEqual(["src/game/Game.h", "src/game/Game.cpp"]);
  });
});
