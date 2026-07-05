/**
 * DOM wiring for the Collatz viewer: the launch form, the episode list, and the per-agent chain
 * display, all refreshed by polling the REST API (no push channel yet).
 *
 * Everything shown about game state is derived client-side by `state.ts` from the raw
 * data-plane feed; lifecycle facts (episode state, live agents) come from the API's
 * control-plane-derived status (ADR-0007).
 */

import { ApiClient, type EpisodeStatus } from "./api.js";
import { deriveChains, parseStarts } from "./state.js";

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
const errorLine = element<HTMLElement>("error");
const episodesBody = element<HTMLTableSectionElement>("episodes");
const detailSection = element<HTMLElement>("detail");
const detailTitle = element<HTMLElement>("detail-title");
const detailState = element<HTMLElement>("detail-state");
const stopButton = element<HTMLButtonElement>("stop");
const chainsContainer = element<HTMLElement>("chains");

let selectedEpisode: string | null = null;

function client(): ApiClient {
  return new ApiClient(apiUrlInput.value.replace(/\/$/, ""));
}

function showError(error: unknown): void {
  errorLine.textContent = error instanceof Error ? error.message : String(error);
}

function clearError(): void {
  errorLine.textContent = "";
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
  episodesBody.replaceChildren(
    ...episodes.map((episode) => {
      const row = document.createElement("tr");
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
      row.addEventListener("click", () => {
        selectedEpisode = episode.episode_id;
        void refresh();
      });
      return row;
    }),
  );
}

function renderDetail(status: EpisodeStatus, chains: ReturnType<typeof deriveChains>): void {
  detailSection.hidden = false;
  detailTitle.textContent = `Episode ${status.episode_id}`;
  detailState.replaceChildren(stateBadge(status.state));
  stopButton.disabled = ["complete", "stopped", "failed"].includes(status.state);
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

async function refresh(): Promise<void> {
  const api = client();
  const episodes = await api.episodes(APPLICATION);
  renderEpisodes(episodes);
  if (selectedEpisode === null) {
    detailSection.hidden = true;
    return;
  }
  const status = episodes.find((episode) => episode.episode_id === selectedEpisode);
  if (status === undefined) {
    detailSection.hidden = true;
    return;
  }
  const records = await api.messages(APPLICATION, selectedEpisode);
  renderDetail(status, deriveChains(records));
}

async function launch(event: SubmitEvent): Promise<void> {
  event.preventDefault();
  clearError();
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
  clearError();
  try {
    await client().stop(APPLICATION, selectedEpisode);
    await refresh();
  } catch (error) {
    showError(error);
  }
}

function poll(): void {
  refresh().then(clearError, showError);
}

launchForm.addEventListener("submit", (event) => void launch(event));
stopButton.addEventListener("click", () => void stopSelected());
apiUrlInput.value = DEFAULT_API_URL;
refreshApplications().then(clearError, showError);
setInterval(poll, POLL_INTERVAL_MS);
