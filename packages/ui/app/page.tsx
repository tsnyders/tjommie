import { PROTOCOL_VERSION } from "@tjommie/protocol";

export default function Home() {
  return (
    <main>
      <div>Tjommie (protocol v{PROTOCOL_VERSION})</div>
    </main>
  );
}
