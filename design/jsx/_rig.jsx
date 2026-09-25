<Frame name="Rig test" w="hug" h="hug" flex="row" gap={24} p={24} bg="#070707">
  <Frame name="Avatar idle" w={80} h={80}>
    <svg name="Body" position="absolute" x={0} y={0} viewBox="0 0 40 40" size={80}><path d="M11 34 C5 34 2 29.5 3.5 25 C2 19 7.5 15 12.5 16.5 C14 10 21 7 26 11 C32 9.5 37 14 35.5 20 C39 23.5 37 32 30.5 34 Z" fill="#EC712E" /></svg>
    <svg name="Eyes" position="absolute" x={0} y={0} viewBox="0 0 40 40" size={80}><path name="Eye L" d="M17.5 19 L16.7 25" stroke="#FFFFFF" strokeWidth={3} fill="none" /><path name="Eye R" d="M24.5 19 L23.7 25" stroke="#FFFFFF" strokeWidth={3} fill="none" /></svg>
  </Frame>
  <Frame name="Avatar look right" w={80} h={80}>
    <svg name="Body" position="absolute" x={0} y={0} viewBox="0 0 40 40" size={80}><path d="M11 34 C5 34 2 29.5 3.5 25 C2 19 7.5 15 12.5 16.5 C14 10 21 7 26 11 C32 9.5 37 14 35.5 20 C39 23.5 37 32 30.5 34 Z" fill="#EC712E" /></svg>
    <svg name="Eyes" position="absolute" x={4} y={0} viewBox="0 0 40 40" size={80}><path d="M17.5 19 L16.7 25" stroke="#FFFFFF" strokeWidth={3} fill="none" /><path d="M24.5 19 L23.7 25" stroke="#FFFFFF" strokeWidth={3} fill="none" /></svg>
  </Frame>
  <Frame name="Avatar closed" w={80} h={80}>
    <svg name="Body" position="absolute" x={0} y={0} viewBox="0 0 40 40" size={80}><path d="M11 34 C5 34 2 29.5 3.5 25 C2 19 7.5 15 12.5 16.5 C14 10 21 7 26 11 C32 9.5 37 14 35.5 20 C39 23.5 37 32 30.5 34 Z" fill="#EC712E" /></svg>
    <svg name="Eyes" position="absolute" x={0} y={0} viewBox="0 0 40 40" size={80}><path d="M15.4 22.5 L18.8 22.5" stroke="#FFFFFF" strokeWidth={2.5} fill="none" /><path d="M22.4 22.5 L25.8 22.5" stroke="#FFFFFF" strokeWidth={2.5} fill="none" /></svg>
  </Frame>
  <ComponentSet name="Agent avatar test">
    <Component name="state=Idle" w={40} h={40}><Ellipse w={40} h={40} bg="#8C5BF3" /></Component>
    <Component name="state=Blink" w={40} h={40}><Ellipse w={40} h={40} bg="#3B82F6" /></Component>
  </ComponentSet>
</Frame>
