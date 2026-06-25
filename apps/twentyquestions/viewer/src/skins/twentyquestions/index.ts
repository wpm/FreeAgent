/**
 * Register the Twenty Questions skin as the right-pane plugin for the
 * "twenty-questions" application. Importing this module for its side effect
 * (done once in `App.svelte`) wires the component into the shell registry.
 */
import { registerPlugin } from "../../shell/registry";
import TwentyQuestions from "./TwentyQuestions.svelte";

registerPlugin("twenty-questions", { component: TwentyQuestions });
