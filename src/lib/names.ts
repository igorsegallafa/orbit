// Session codenames (adjective-noun, Docker-style) so sidebar entries are
// distinguishable instead of three tabs all called "claude".
const ADJECTIVES = [
  "amber", "bold", "calm", "daring", "eager", "fierce", "gentle", "jolly",
  "keen", "lunar", "nimble", "quiet", "rapid", "solar", "swift", "tidal",
  "ultra", "vivid", "witty", "zesty",
];

const NOUNS = [
  "comet", "falcon", "lynx", "meteor", "nebula", "nova", "otter", "pegasus",
  "phoenix", "quasar", "quokka", "raven", "rocket", "sparrow", "sputnik",
  "tiger", "voyager", "panda", "orion", "magnetar",
];

export function randomSessionName(taken: Iterable<string>): string {
  const used = new Set(taken);
  for (let i = 0; i < 50; i++) {
    const name = `${ADJECTIVES[Math.floor(Math.random() * ADJECTIVES.length)]}-${
      NOUNS[Math.floor(Math.random() * NOUNS.length)]
    }`;
    if (!used.has(name)) return name;
  }
  return `session-${Date.now() % 10000}`;
}