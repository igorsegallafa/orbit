import { describe, expect, it } from "vitest";
import { parseProblem } from "./problems";

describe("parseProblem", () => {
  it("reads gcc and clang diagnostics", () => {
    expect(parseProblem("/src/engine/src/game/Game.cpp:42:7: error: 'foo' was not declared in this scope")).toEqual({
      file: "/src/engine/src/game/Game.cpp",
      line: 42,
      column: 7,
      severity: "error",
      code: undefined,
      message: "'foo' was not declared in this scope",
    });
    const w = parseProblem("\x1b[1m../../src/game/Game.h:10:3: \x1b[35mwarning: \x1b[0munused variable 'x' [-Wunused-variable]", "/src/engine/build/linux");
    expect(w).toMatchObject({ file: "/src/engine/src/game/Game.h", line: 10, column: 3, severity: "warning", code: "-Wunused-variable", message: "unused variable 'x'" });
    expect(parseProblem("x.cpp:3: fatal error: y.h: No such file or directory")).toMatchObject({ line: 3, column: 1, severity: "error" });
  });

  it("reads msvc diagnostics", () => {
    expect(parseProblem("C:\\src\\engine\\src\\game\\Game.cpp(42,7): error C2065: 'foo': undeclared identifier")).toEqual({
      file: "C:\\src\\engine\\src\\game\\Game.cpp",
      line: 42,
      column: 7,
      severity: "error",
      code: "C2065",
      message: "'foo': undeclared identifier",
    });
    expect(parseProblem("  Game.cpp(9): warning C4996: 'strcpy': This function may be unsafe.", "C:\\b")).toMatchObject({ file: "C:\\b\\Game.cpp", line: 9, severity: "warning" });
  });

  it("reads linker errors without a location", () => {
    expect(parseProblem("/usr/bin/ld: Game.cpp.o: in function `Game::Init()': undefined reference to `Foo::Bar()'")).toMatchObject({ file: null, severity: "error" });
    expect(parseProblem("LINK : fatal error LNK1104: cannot open file 'game.exe'")).toMatchObject({ file: null, code: "LNK1104" });
  });

  it("ignores everything else", () => {
    expect(parseProblem("[12/340] Building CXX object src/game/CMakeFiles/game.dir/Game.cpp.o")).toBeNull();
    expect(parseProblem("In file included from /src/a.h:3:")).toBeNull();
    expect(parseProblem("/src/a.cpp:5:3: note: declared here")).toBeNull();
  });
});
