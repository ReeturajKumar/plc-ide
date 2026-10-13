import { runtimeApi } from "../services/runtimeApi";
import type { Tag } from "../types/protocol";

// The SCADA tags by upper-cased name, loaded from the backend on first use. A failed
// load (backend or tag database down) is retried on the next use, at most every 10 s.
// ponytail: loaded once per app start; add a refresh when tags change while it runs.
let loaded: Promise<Map<string, Tag>> | null = null;
let failedAt = 0;

function tags(): Promise<Map<string, Tag>> {
    if (!loaded && Date.now() - failedAt > 10_000) {
        loaded = runtimeApi.getTags().then(
            (list) => new Map(list.map((t) => [t.name.toUpperCase(), t])),
            (e) => {
                console.warn("[tags]", e instanceof Error ? e.message : e);
                loaded = null;
                failedAt = Date.now();
                return new Map();
            },
        );
    }
    return loaded ?? Promise.resolve(new Map());
}

/** Every tag (none if the backend or tag database can't be reached). */
export async function tagList(): Promise<Tag[]> {
    return [...(await tags()).values()];
}

/** The tag named `name` (case-insensitive, like every ST name), if there is one. */
export async function findTag(name: string): Promise<Tag | undefined> {
    return (await tags()).get(name.toUpperCase());
}

/** Hover text for a tag: Markdown sections. */
export function tagHover(tag: Tag): string[] {
    const scale =
        tag.scaleFactor === 1 && tag.scaleOffset === 0
            ? "none (raw value)"
            : `raw × ${tag.scaleFactor}${tag.scaleOffset ? ` + ${tag.scaleOffset}` : ""}`;
    const line = (label: string, value: string) => (value ? `**${label}:** ${value}  \n` : "");
    return [
        `**${tag.name}** — SCADA tag`,
        tag.description || "_No description_",
        line("Type in ST", `\`${tag.stType}\` (SCADA: ${tag.scadaType || "?"})`) +
            line("Class", tag.tagClass) +
            line("Unit", tag.unit) +
            line("Device", [tag.device, tag.protocol].filter(Boolean).join(" · ")) +
            line("Address", tag.address) +
            line("Scaling", scale),
    ];
}
