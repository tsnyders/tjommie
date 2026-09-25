<Frame name="Smoke test" w={640} h="hug" flex="col" gap={16} p={24} bg="#070707">
  <Frame name="Row" flex="row" gap={12} items="center">
    <Ellipse name="Avatar circle" w={40} h={40} bg="#8C5BF3" />
    <svg name="Blob cloud" viewBox="0 0 48 40" size={40}><path d="M12 38 C4 38 0 32 2 26 C0 18 8 12 14 14 C16 6 26 2 32 8 C40 6 46 12 44 20 C48 26 44 36 36 38 Z" fill="#EC4698" /><path d="M20 16 L19 22" stroke="#FFFFFF" strokeWidth={3} fill="none" /><path d="M28 16 L27 22" stroke="#FFFFFF" strokeWidth={3} fill="none" /></svg>
    <Polygon name="Hex" pointCount={6} w={40} h={40} bg="#EC712E" cornerRadius={6} />
    <Icon name="lucide:monitor" size={20} color="#ACACAC" />
    <Icon name="lucide:settings" size={20} color="#ACACAC" />
    <Text name="Label" size={15} font="Segoe UI" weight="semibold" color="#FCFCFC">Pnet apply</Text>
  </Frame>
  <Frame name="Bubble" w={420} h="hug" p={14} bg="#262626" rounded={18} shadow="0 1 2 #00000040">
    <Text name="Body" w="fill" size={14} lineHeight={21} font="Segoe UI" color="#F5F5F5">The morning window closed early with no new applications. You stayed signed in, but every listing left was one you've already applied to.</Text>
  </Frame>
  <Frame name="Pill" flex="row" gap={6} px={12} py={6} bg="#459FFF" rounded={999} items="center">
    <Icon name="lucide:arrow-up" size={14} color="#FFFFFF" />
    <Text size={13} font="Segoe UI" weight="semibold" color="#FFFFFF">22 new messages</Text>
  </Frame>
</Frame>
