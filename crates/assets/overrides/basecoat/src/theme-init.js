import { applyMode, readMode } from "./appearance.js";

// This small blocking head script sets the palette before the stylesheet's first paint.
applyMode(document, readMode(window), window.matchMedia("(prefers-color-scheme: dark)").matches);
