export function createComputersPage(): HTMLElement {
	const page = document.createElement('main');
	// This static markup contains no engine/device data. Future labels use textContent.
	page.innerHTML = `<header class="page-heading"><h1>Your computers</h1><p>A place for the computers you connect to.</p></header>
    <section class="empty-state" aria-label="Computers">
      <svg class="computer" viewBox="0 0 64 64" fill="none" aria-hidden="true"><rect x="8" y="10" width="48" height="32" rx="4"/><path d="M32 42v12M22 54h20"/></svg>
      <h2>No computers yet</h2><p>Paired computers will appear here.</p>
    </section>`;
	return page;
}
