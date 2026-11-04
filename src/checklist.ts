// A short routine to run before each move, written for players around
// 25 kyu to 5 kyu. Safety first, then the opponent's move, then chances,
// then the whole board. Ticks reset after every move.

export type ChecklistSize = 9 | 13 | 19;

export const CHECKLISTS: Record<ChecklistSize, { title: string; items: string[] }> = {
  9: {
    title: "Before you move, 9 × 9",
    items: [
      "Liberties: is any group, mine or theirs, in atari or down to two?",
      "Their last move: what does it threaten?",
      "Can I cut their stones apart, or must I connect mine?",
      "Will my group have room for two eyes?",
      "Count: if I'm ahead, play safe; if behind, start a fight.",
    ],
  },
  13: {
    title: "Before you move, 13 × 13",
    items: [
      "Are any of my stones in atari or short of liberties?",
      "Their last move: what does it threaten?",
      "Can I capture, cut, or chase a weak group?",
      "Do my groups have a base, with room for two eyes?",
      "Where is the biggest open area left?",
    ],
  },
  19: {
    title: "Before you move, 19 × 19",
    items: [
      "Are any of my groups in atari or short of liberties?",
      "Their last move: what does it threaten? Does it need an answer?",
      "Can I capture or cut something of theirs?",
      "Is my weakest group getting stronger or weaker?",
      "Is there a bigger move elsewhere: corners, then sides, then center?",
    ],
  },
};

/** The checklist for a board size; odd sizes use the nearest standard one. */
export function checklistFor(size: number): { size: ChecklistSize; title: string; items: string[] } {
  const s: ChecklistSize = size <= 11 ? 9 : size <= 16 ? 13 : 19;
  return { size: s, ...CHECKLISTS[s] };
}

let shownFor = "";

/**
 * Draws the checklist into `host`. `key` identifies the position: when it
 * changes (a move was played), every tick clears.
 */
export function renderChecklist(host: HTMLElement, size: number, key: string) {
  const list = checklistFor(size);
  const fresh = key !== shownFor;
  shownFor = key;
  let section = host.querySelector<HTMLElement>(".checklist");
  if (!section || section.dataset.size !== String(list.size)) {
    section?.remove();
    section = document.createElement("section");
    section.className = "checklist";
    section.dataset.size = String(list.size);
    section.setAttribute("aria-label", list.title);
    const h = document.createElement("p");
    h.className = "checklist-title";
    h.textContent = list.title;
    const ul = document.createElement("ul");
    list.items.forEach((text, i) => {
      const li = document.createElement("li");
      const id = `check-${list.size}-${i}`;
      const box = document.createElement("input");
      box.type = "checkbox";
      box.id = id;
      const label = document.createElement("label");
      label.htmlFor = id;
      label.textContent = text;
      li.append(box, label);
      ul.append(li);
    });
    section.append(h, ul);
  }
  if (section.parentElement !== host) host.append(section);
  if (fresh) section.querySelectorAll<HTMLInputElement>("input").forEach((b) => (b.checked = false));
}

export function hideChecklist() {
  document.querySelector(".checklist")?.remove();
  shownFor = "";
}
