import base from "/home/user/wenlan-step2/playwright.config";
const shell = "/opt/pw-browsers/chromium_headless_shell-1243/chrome-headless-shell-linux64/chrome-headless-shell";
const cfg: any = { ...base, projects: base.projects!.map((p: any) => ({ ...p, use: { ...p.use, launchOptions: { executablePath: shell, args: ["--no-sandbox"] } } })) };
export default cfg;
