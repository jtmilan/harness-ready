// Random, easy-to-recognize agent codenames (e.g. "CRIMSON FALCON").
const ADJECTIVES = ["CRIMSON", "AMBER", "COBALT", "ONYX", "SILVER", "NEON", "SOLAR", "LUNAR", "RAPID", "SILENT", "IRON", "TURBO", "PHANTOM", "PRIME", "ZERO", "NOVA", "HYPER", "ROGUE", "STATIC", "QUANTUM"];
const NOUNS = ["FALCON", "VIPER", "OTTER", "RAVEN", "MANTIS", "LYNX", "COBRA", "BADGER", "HERON", "WOLF", "GECKO", "ORCA", "HAWK", "PANTHER", "MOTH", "BISON", "CONDOR", "JACKAL", "TIGER", "OWL"];

const used = new Set();

export function randomAgentName() {
  for (let i = 0; i < 40; i++) {
    const name = `${ADJECTIVES[Math.floor(Math.random() * ADJECTIVES.length)]} ${NOUNS[Math.floor(Math.random() * NOUNS.length)]}`;
    if (!used.has(name)) {
      used.add(name);
      return name;
    }
  }
  return `AGENT ${Math.floor(Math.random() * 900) + 100}`;
}