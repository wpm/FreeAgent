/**
 * DOM wiring for the Collatz viewer: the launch form, the episode list, and the per-agent chain
 * display, all refreshed by polling the REST API (no push channel yet).
 *
 * Everything shown about game state is derived client-side by `state.ts` from the raw
 * data-plane feed; lifecycle facts (episode state, live agents) come from the API's
 * control-plane-derived status.
 *
 * Polling discipline: the next poll is scheduled only after the current one settles (no
 * overlapping fetches, no out-of-order paints), the message feed is refetched only when the
 * episode's `message_count` says it grew, and the DOM is rebuilt only when what it would show
 * actually changed — so clicks, text selection, and hover states survive quiet ticks.
 *
 * Errors — from user actions and the background poll alike — surface as toasts (issue #118):
 * each stays until the user dismisses it (a later success never clears it), nothing is persisted,
 * and repeats of an identical error coalesce into the one toast per `toast.ts`'s store.
 */

import { ApiClient, TERMINAL_STATES, type EpisodeStatus } from "./api.js";
import { deriveChains, parseStarts, type AgentChain } from "./state.js";
import { formatToast, ToastStore } from "./toast.js";

const APPLICATION = "collatz";
const POLL_INTERVAL_MS = 500;
const DEFAULT_API_URL = "http://127.0.0.1:8000";

function element<T extends HTMLElement>(id: string): T {
  const found = document.getElementById(id);
  if (found === null) {
    throw new Error(`Missing element #${id}`);
  }
  return found as T;
}

const apiUrlInput = element<HTMLInputElement>("api-url");
const applicationsList = element<HTMLElement>("applications");
const launchForm = element<HTMLFormElement>("launch-form");
const episodeIdInput = element<HTMLInputElement>("episode-id");
const startsInput = element<HTMLInputElement>("starts");
const toastsContainer = element<HTMLElement>("toasts");
const episodesBody = element<HTMLTableSectionElement>("episodes");
const detailSection = element<HTMLElement>("detail");
const detailTitle = element<HTMLElement>("detail-title");
const detailState = element<HTMLElement>("detail-state");
const stopButton = element<HTMLButtonElement>("stop");
const chainsContainer = element<HTMLElement>("chains");

let selectedEpisode: string | null = null;

/** The last-rendered episode rows, as JSON, to skip DOM rebuilds on quiet polls. */
let renderedEpisodes = "";

/** The last-rendered detail pane, as JSON, for the same reason. */
let renderedDetail = "";

/** Cache of the selected episode's derived chains, valid while `message_count` holds still. */
let chainsCache: { episodeId: string; messageCount: number; chains: AgentChain[] } | null = null;

const toastStore = new ToastStore();

/** Each live toast's occurrence counter, for updating in place when the store coalesces. */
const toastCounters = new Map<number, HTMLElement>();

function client(): ApiClient {
  return new ApiClient(apiUrlInput.value.replace(/\/$/, ""));
}

/**
 * Surface an error as a toast that stays until the user dismisses it.
 *
 * The one error sink for actions and the poll alike — with a toast per error there is no shared
 * line to fight over, so the old poll/action ownership bookkeeping is gone. An error identical
 * to a toast still showing bumps that toast's counter instead of stacking a duplicate (the
 * counter is `aria-hidden`: announcing every poll-tick repeat would spam screen readers, so a
 * repeat is visual only). Nothing else removes a toast — not a later success, not a timer.
 */
function showError(error: unknown): void {
  const event = toastStore.add(formatToast(error));
  if (event.kind === "repeated") {
    const counter = toastCounters.get(event.id);
    if (counter !== undefined) {
      counter.textContent = `× ${event.count}`;
    }
    return;
  }
  const toast = document.createElement("div");
  toast.className = "toast";
  toast.setAttribute("role", "alert");
  const text = document.createElement("div");
  text.className = "text";
  if (event.content.title !== null) {
    const title = document.createElement("p");
    title.className = "title";
    title.textContent = event.content.title;
    text.append(title);
  }
  const body = document.createElement("p");
  body.className = "body";
  body.textContent = event.content.body;
  text.append(body);
  const counter = document.createElement("span");
  counter.className = "count";
  counter.setAttribute("aria-hidden", "true");
  const dismiss = document.createElement("button");
  dismiss.type = "button";
  dismiss.className = "dismiss";
  dismiss.setAttribute("aria-label", "Dismiss");
  dismiss.textContent = "×";
  dismiss.addEventListener("click", () => {
    toastStore.dismiss(event.id);
    toastCounters.delete(event.id);
    toast.remove();
  });
  toast.append(text, counter, dismiss);
  toastCounters.set(event.id, counter);
  toastsContainer.append(toast);
}

async function refreshApplications(): Promise<void> {
  const applications = await client().applications();
  applicationsList.textContent = applications.join(", ");
}

function stateBadge(state: EpisodeStatus["state"]): HTMLElement {
  const badge = document.createElement("span");
  badge.className = `badge state-${state}`;
  badge.textContent = state;
  return badge;
}

function renderEpisodes(episodes: EpisodeStatus[]): void {
  const snapshot = JSON.stringify([episodes, selectedEpisode]);
  if (snapshot === renderedEpisodes) {
    return;
  }
  renderedEpisodes = snapshot;
  episodesBody.replaceChildren(
    ...episodes.map((episode) => {
      const row = document.createElement("tr");
      row.dataset["episodeId"] = episode.episode_id;
      if (episode.episode_id === selectedEpisode) {
        row.className = "selected";
      }
      const id = document.createElement("td");
      id.textContent = episode.episode_id;
      const state = document.createElement("td");
      state.append(stateBadge(episode.state));
      const agents = document.createElement("td");
      agents.textContent = episode.agents_alive.join(", ") || "—";
      const messages = document.createElement("td");
      messages.textContent = String(episode.message_count);
      row.append(id, state, agents, messages);
      return row;
    }),
  );
}

function renderDetail(status: EpisodeStatus, chains: AgentChain[]): void {
  const snapshot = JSON.stringify([status, chains]);
  if (!detailSection.hidden && snapshot === renderedDetail) {
    return;
  }
  renderedDetail = snapshot;
  detailSection.hidden = false;
  detailTitle.textContent = `Episode ${status.episode_id}`;
  detailState.replaceChildren(stateBadge(status.state));
  stopButton.disabled = TERMINAL_STATES.has(status.state);
  chainsContainer.replaceChildren(
    ...chains.map((chain) => {
      const card = document.createElement("div");
      card.className = "chain";
      const name = document.createElement("h3");
      name.textContent = chain.agent;
      const alive = status.agents_alive.includes(chain.agent);
      const badge = document.createElement("span");
      // An agent that reached 1 was stopped by the environment (StopAgent), so "done" here is
      // the visible face of that control-plane fact; "running" mirrors agents_alive.
      badge.className = chain.complete ? "badge done" : alive ? "badge running" : "badge";
      badge.textContent = chain.complete ? "done" : alive ? "running" : "stopped";
      name.append(" ", badge);
      const numbers = document.createElement("p");
      numbers.textContent = chain.numbers.join(" → ");
      const steps = document.createElement("p");
      steps.className = "steps";
      steps.textContent = `${chain.numbers.length - 1} steps`;
      card.append(name, numbers, steps);
      return card;
    }),
  );
}

/** The selected episode's chains, refetching the feed only when it has grown. */
async function selectedChains(api: ApiClient, status: EpisodeStatus): Promise<AgentChain[]> {
  if (
    chainsCache !== null &&
    chainsCache.episodeId === status.episode_id &&
    chainsCache.messageCount === status.message_count
  ) {
    return chainsCache.chains;
  }
  const records = await api.messages(APPLICATION, status.episode_id);
  const chains = deriveChains(records);
  chainsCache = {
    episodeId: status.episode_id,
    messageCount: status.message_count,
    chains,
  };
  return chains;
}

async function refresh(): Promise<void> {
  const api = client();
  const episodes = await api.episodes(APPLICATION);
  renderEpisodes(episodes);
  const status = episodes.find((episode) => episode.episode_id === selectedEpisode);
  if (status === undefined) {
    detailSection.hidden = true;
    return;
  }
  renderDetail(status, await selectedChains(api, status));
}

async function launch(event: SubmitEvent): Promise<void> {
  event.preventDefault();
  try {
    const starts = parseStarts(startsInput.value);
    if (starts.length === 0) {
      throw new Error("Give at least one starting number");
    }
    const episodeId = episodeIdInput.value.trim();
    const created = await client().createEpisode(APPLICATION, episodeId || null, { starts });
    selectedEpisode = created.episode_id;
    episodeIdInput.value = "";
    await refresh();
  } catch (error) {
    showError(error);
  }
}

async function stopSelected(): Promise<void> {
  if (selectedEpisode === null) {
    return;
  }
  try {
    await client().stop(APPLICATION, selectedEpisode);
    await refresh();
  } catch (error) {
    showError(error);
  }
}

/** Run one poll, then schedule the next — only after this one settles, so polls never overlap. */
function poll(): void {
  void refresh()
    .catch(showError)
    .finally(() => setTimeout(poll, POLL_INTERVAL_MS));
}

launchForm.addEventListener("submit", (event) => void launch(event));
stopButton.addEventListener("click", () => void stopSelected());
// One delegated listener: rows are rebuilt when the table changes, so per-row listeners would
// miss a click that straddles a rebuild.
episodesBody.addEventListener("click", (event) => {
  const row = (event.target as HTMLElement).closest("tr");
  const episodeId = row?.dataset["episodeId"];
  if (episodeId !== undefined) {
    selectedEpisode = episodeId;
    refresh().catch(showError);
  }
});
apiUrlInput.value = DEFAULT_API_URL;
apiUrlInput.addEventListener("change", () => {
  refreshApplications().catch(showError);
});
refreshApplications().catch(showError);
poll();
