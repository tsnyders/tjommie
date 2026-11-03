/** One host-owned controller and privacy epoch; never derive ownership from client parameters. */
export class NativeBrowserAccess {
  owner: string | null = null;
  controlEpoch = 0;
  privateEpoch = 0;
  privateActive = false;

  get control(): "human" | "bot" { return this.owner !== null || this.privateActive ? "human" : "bot"; }
  take(owner: string): void {
    if (this.owner !== null && this.owner !== owner) throw new Error("Another device is controlling this browser.");
    if (this.owner === owner) return;
    this.owner = owner; this.controlEpoch++;
  }
  release(owner: string): boolean {
    if (this.owner !== owner) return false;
    this.owner = null; this.controlEpoch++; return true;
  }
  require(owner: string, epoch: number): void {
    if (this.owner !== owner || this.controlEpoch !== epoch) throw new Error("Browser control changed. Take control again and use the current frame.");
  }
  changedPage(): void { this.controlEpoch++; }
  ticket(): { controlEpoch: number; privateEpoch: number } { return { controlEpoch: this.controlEpoch, privateEpoch: this.privateEpoch }; }
  check(ticket: { controlEpoch: number; privateEpoch: number }, automation = false): void {
    if (this.privateActive || ticket.privateEpoch !== this.privateEpoch || ticket.controlEpoch !== this.controlEpoch) throw new Error("The browser interaction changed; this result was discarded.");
    if (automation && this.control !== "bot") throw new Error("The person is controlling this browser. Wait until they return control.");
  }
  beginPrivate(owner: string, epoch: number): number {
    this.require(owner, epoch);
    this.privateActive = true; this.privateEpoch++; this.controlEpoch++;
    return this.privateEpoch;
  }
  endPrivate(owner: string, epoch: number): void {
    if (!this.privateActive || this.owner !== owner || this.privateEpoch !== epoch) throw new Error("The private interaction changed.");
    this.privateActive = false; this.privateEpoch++; this.controlEpoch++;
  }
}
