const attributes = {
  path: new Set(["d"]),
  circle: new Set(["cx", "cy", "r", "fill"]),
  rect: new Set(["x", "y", "width", "height", "rx", "ry"]),
  line: new Set(["x1", "x2", "y1", "y2"]),
  ellipse: new Set(["cx", "cy", "rx", "ry"]),
  polyline: new Set(["points"]),
  polygon: new Set(["points"]),
};

function geometry(name, nodes) {
  if (!Array.isArray(nodes) || nodes.length === 0) throw new Error(`Invalid Lucide geometry: ${name}`);
  return nodes.map(node => {
    if (!Array.isArray(node) || node.length !== 2) throw new Error(`Invalid Lucide node: ${name}`);
    const [tag, props] = node;
    if (!Object.hasOwn(attributes, tag) || !props || typeof props !== "object" || Array.isArray(props)) {
      throw new Error(`Unsupported Lucide node: ${name}/${tag}`);
    }
    const rendered = Object.entries(props).map(([attribute, value]) => {
      const text = String(value);
      const valid = attribute === "fill"
        ? ["none", "currentColor"].includes(text)
        : attribute === "d"
          ? /^[MmLlHhVvCcSsQqTtAaZz0-9eE.,+\-\s]+$/.test(text)
          : /^[0-9eE.,+\-\s]+$/.test(text);
      if (!attributes[tag].has(attribute) || !["string", "number"].includes(typeof value) || !valid) {
        throw new Error(`Unsupported Lucide attribute: ${name}/${tag}/${attribute}`);
      }
      return `${attribute}="${text}"`;
    }).join(" ");
    return `<${tag} ${rendered}/>`;
  }).join("");
}

// icon-nodes.json contains canonical icons only; icons/ also contains historical aliases.
// Construct SVG from an explicit geometric allowlist rather than accepting arbitrary markup.
export function buildLucideCatalog(nodes) {
  return Object.keys(nodes).sort().map(name => {
    if (!/^[a-z0-9]+(?:-[a-z0-9]+)*$/.test(name)) throw new Error(`Invalid Lucide name: ${name}`);
    const label = name.split("-").map(word => word[0].toUpperCase() + word.slice(1)).join(" ");
    const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="lucide lucide-${name}" aria-hidden="true" focusable="false">${geometry(name, nodes[name])}</svg>`;
    return { name, label, svg };
  });
}
