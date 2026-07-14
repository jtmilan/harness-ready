/** Local Storage key for AgentTemplate entities (offline stand-in for Base44). */
const STORAGE_KEY = "agent-templates";

/**
 * @typedef {{ role: string, kind?: string, priority?: string, autonomy?: string }} AgentConfig
 * @typedef {{
 *   id: string,
 *   name: string,
 *   description?: string,
 *   agents: AgentConfig[],
 *   created_date: string,
 *   updated_date: string,
 * }} AgentTemplate
 */

function readAll() {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return [];
    const parsed = JSON.parse(raw);
    return Array.isArray(parsed) ? parsed : [];
  } catch {
    return [];
  }
}

function writeAll(templates) {
  localStorage.setItem(STORAGE_KEY, JSON.stringify(templates));
}

function generateId() {
  if (typeof crypto !== "undefined" && typeof crypto.randomUUID === "function") {
    return crypto.randomUUID();
  }
  return `tpl_${Date.now()}_${Math.random().toString(36).slice(2, 10)}`;
}

/**
 * Sort key starting with `-` means descending (Base44 SDK convention).
 * @param {AgentTemplate[]} items
 * @param {string} [order]
 */
function applySort(items, order) {
  if (!order || typeof order !== "string") return items;
  const descending = order.startsWith("-");
  const field = descending ? order.slice(1) : order;
  return [...items].sort((a, b) => {
    const av = a[field] ?? "";
    const bv = b[field] ?? "";
    if (av < bv) return descending ? 1 : -1;
    if (av > bv) return descending ? -1 : 1;
    return 0;
  });
}

export const AgentTemplate = {
  /**
   * @param {string} [order] e.g. "-created_date"
   * @returns {Promise<AgentTemplate[]>}
   */
  async list(order) {
    return applySort(readAll(), order);
  },

  /**
   * @param {Record<string, unknown>} [criteria]
   * @param {string} [order]
   * @returns {Promise<AgentTemplate[]>}
   */
  async filter(criteria = {}, order) {
    const entries = Object.entries(criteria);
    const filtered = readAll().filter((item) =>
      entries.every(([key, value]) => item[key] === value)
    );
    return applySort(filtered, order);
  },

  /**
   * @param {{ name: string, agents: AgentConfig[], description?: string }} data
   * @returns {Promise<AgentTemplate>}
   */
  async create(data) {
    const now = new Date().toISOString();
    const template = {
      id: generateId(),
      name: data.name,
      description: data.description ?? "",
      agents: Array.isArray(data.agents) ? data.agents : [],
      created_date: now,
      updated_date: now,
    };
    const all = readAll();
    all.push(template);
    writeAll(all);
    return template;
  },

  /**
   * @param {string} id
   * @param {Partial<AgentTemplate>} data
   * @returns {Promise<AgentTemplate>}
   */
  async update(id, data) {
    const all = readAll();
    const index = all.findIndex((t) => t.id === id);
    if (index === -1) {
      throw new Error(`AgentTemplate not found: ${id}`);
    }
    const updated = {
      ...all[index],
      ...data,
      id: all[index].id,
      created_date: all[index].created_date,
      updated_date: new Date().toISOString(),
    };
    all[index] = updated;
    writeAll(all);
    return updated;
  },

  /**
   * @param {string} id
   * @returns {Promise<void>}
   */
  async delete(id) {
    writeAll(readAll().filter((t) => t.id !== id));
  },
};
