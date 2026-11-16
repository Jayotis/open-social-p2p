# Future ideas

These are ideas worth keeping, but they are not requirements for the current Product Alpha or the first prototype that follows it. Move one into the roadmap or an ADR only when real use or testing gives us a reason to build it.

## Shared community governance

The first public-community prototype should have one owner and accepted co-moderators. That is enough to learn how people actually create, use and moderate communities.

If real communities outgrow that model, revisit:

- several stewards sharing control instead of one owner;
- majority approval for permanent community bans;
- shared approval for adding/removing stewards and changing important rules;
- independent appeals;
- succession when stewards leave;
- a recovery plan when too few stewards remain;
- what to do if two different signed governance histories appear.

Do not pick thresholds or recovery delays until there is real community use to test them against.

## Prebuilt node hardware

OSP may later offer prebuilt boxes based on the capability profiles in [doc 18](../docs/18-node-capability-profiles.md):

- **Home:** ordinary always-on personal node;
- **Library:** more storage/discovery/distribution work;
- **Studio:** stronger media creation/transcoding/serving.

Possible vendor relationships, bulk SBC purchases and crowdfunding belong here until the software workloads and Product Alpha usage are measured.

Buying a stronger box must never grant social rank, feed priority or community authority.

## App navigation after the Product Alpha

[ADR 0031](../docs/decisions/0031-app-shell-home-universe-you.md) currently defines three places: Home, Universe and You.

Before Front Page or Media changes the shell, decide from real use whether:

- Media is a mode inside Home;
- Media is its own place;
- Front Page is its own place;
- the shell grows to five places;
- another layout works better.

Do not make later planning documents silently supersede ADR 0031.
