import base from "/home/user/wenlan-step2/playwright.config";
const cfg: any = { ...base, projects: base.projects!.map((p: any) => ({ ...p, use: { ...p.use, launchOptions: { executablePath: "/opt/pw-browsers/chromium", args: ["--no-sandbox"] } } })) };
export default cfg;
