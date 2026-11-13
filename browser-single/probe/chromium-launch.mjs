// Playwright disables the Chromium sandbox by default; opt in explicitly.
export function chromiumLaunchOptions(executablePath, noSandbox = false) {
  return {
    executablePath,
    headless: true,
    chromiumSandbox: !noSandbox,
    args: [
      '--disable-dev-shm-usage',
      '--disable-background-timer-throttling',
      ...(noSandbox ? ['--no-sandbox'] : []),
    ],
  };
}
