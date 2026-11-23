import { invoke } from "@tauri-apps/api/core";

/** Launch game (Android via Kotlin plugin starts GameActivity; desktop runs local binary). */
export const launcher = {
  launchGame: (config: string) => invoke<void>("launch_game", { config }),
};

export function errorText(error: unknown): string {
  if (typeof error === "string") return error;
  if (error instanceof Error) return error.message;
  return String(error);
}
