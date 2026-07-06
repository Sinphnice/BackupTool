import { invoke } from "@tauri-apps/api/core";
import "./styles.css";

const button = document.querySelector<HTMLButtonElement>("#test-core");
const result = document.querySelector<HTMLParagraphElement>("#core-result");

if (!button || !result) {
  throw new Error("Missing required UI elements");
}

button.addEventListener("click", async () => {
  result.textContent = "Calling C++ core...";

  try {
    result.textContent = await invoke<string>("core_version");
  } catch (error) {
    result.textContent = String(error);
  }
});
