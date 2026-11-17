/// The terms of use (ADR 0042 §3), agreed before pairing. The same words
/// are in docs/terms.md; a test keeps the two the same.
library;

/// Raised whenever [termsOfUse] changes, so each phone shows them again,
/// once.
const termsVersion = 1;

const termsOfUse = [
  'Post only what you made, or have the right to share.',
  'You answer for what you post. Posts are public, other people\'s nodes '
      'keep copies, and deleting a post reaches the nodes that cooperate, '
      'not copies someone saved.',
  'Never permitted, with no tolerance in OSP\'s official apps and '
      'software: explicit sexual material; prolonged graphic or sadistic '
      'realistic violence; any sexual material involving minors; intimate '
      'images shared without the consent of the person in them; credible '
      'threats of violence against a person, and material made to incite it.',
  'Anyone can report a post or person from the app. The project reviews '
      'every report, and official OSP software hides what breaks these '
      'rules.',
  'Your node is yours. There\'s no account with the project, and the '
      'project can\'t change what\'s on your node.',
];
