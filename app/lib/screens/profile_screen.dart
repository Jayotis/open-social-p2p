import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../format.dart';
import '../node.dart';
import '../blocking.dart';
import '../report.dart';
import '../osp_codes.dart';
import '../src/rust/api/osp.dart';
import '../theme.dart';
import '../widgets/person_avatar.dart';
import '../widgets/sky_band.dart';
import '../widgets/post_card.dart';
import 'edit_profile_screen.dart';
import 'image_screen.dart';
import 'people_screen.dart' show AskTrustDialog;
import 'player_screen.dart';
import 'trust_screen.dart' show confirmTrust, shownName;
import '../widgets/empty_state.dart';
import '../widgets/sky_bar.dart';

/// Someone's profile (ADR 0032 §4): picture, name, summary and links, then
/// their posts, a grid of their photos and videos, and their reshares, as
/// far as your node holds them. Follow, and say you know them, from here.
class ProfileScreen extends StatefulWidget {
  const ProfileScreen({
    super.key,
    required this.node,
    required this.identity,
    this.name = '',
  });

  final HomeNode node;
  final String identity;

  /// Their name as the screen that opened this showed it, until the
  /// profile arrives.
  final String name;

  @override
  State<ProfileScreen> createState() => _ProfileScreenState();
}

/// The three lists a profile shows.
enum ProfileList {
  posts('Posts'),
  media('Media'),
  reshares('Reshares');

  const ProfileList(this.label);
  final String label;
}

/// One list, as far as it's been read.
class _Listed {
  final posts = <Post>[];
  String? next;
  bool done = false;
  bool loading = false;
  String? error;
}

class _ProfileScreenState extends State<ProfileScreen> {
  /// Most items asked for at once; the node gives at most 50.
  static const _page = 30;

  Profile? _profile;
  String? _error;
  bool _busy = false;
  ProfileList _list = ProfileList.posts;
  final Map<ProfileList, _Listed> _lists = {
    for (final list in ProfileList.values) list: _Listed(),
  };

  HomeNode get _node => widget.node;

  @override
  void initState() {
    super.initState();
    _load();
  }

  Future<void> _load() async {
    try {
      final profile = await _node.app.profile(identity: widget.identity);
      if (!mounted) return;
      setState(() {
        _profile = profile;
        _error = null;
      });
      if (_lists[_list]!.posts.isEmpty) await _more();
    } catch (e) {
      if (mounted) setState(() => _error = describeError(e));
    }
  }

  /// Reads the next page of the list shown.
  Future<void> _more() async {
    final list = _list;
    final listed = _lists[list]!;
    if (listed.loading || listed.done) return;
    setState(() {
      listed.loading = true;
      listed.error = null;
    });
    try {
      final page = await _node.app.profileItems(
        identity: widget.identity,
        kind: list.name,
        before: listed.next,
        limit: _page,
      );
      listed.posts.addAll(page.posts);
      listed.next = page.next;
      listed.done = page.next == null;
    } catch (e) {
      listed.error = describeError(e);
    } finally {
      listed.loading = false;
    }
    if (mounted) setState(() {});
  }

  /// Starts the lists again from the newest, after a follow, say.
  Future<void> _reload() async {
    for (final list in ProfileList.values) {
      _lists[list] = _Listed();
    }
    await _load();
  }

  /// Runs [action], then says [done], or why it failed.
  Future<void> _do(Future<void> Function() action, String done) async {
    final messenger = ScaffoldMessenger.of(context);
    setState(() => _busy = true);
    try {
      await action();
      messenger.showSnackBar(SnackBar(content: Text(done)));
      await _load();
    } catch (e) {
      messenger.showSnackBar(SnackBar(content: Text(describeError(e))));
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  String get _name => shownName(_profile?.name ?? widget.name);

  Future<void> _follow() => _do(
    () => _node.app.followPerson(identity: widget.identity),
    'Following $_name. Their posts reach Home once their node answers.',
  );

  Future<void> _unfollow() => _do(
    () => _node.app.unfollow(identity: widget.identity),
    'You no longer follow $_name.',
  );

  Future<void> _trust() async {
    if (!await confirmTrust(context, _name)) return;
    await _do(
      () => _node.app.trust(identity: widget.identity),
      'You know $_name.',
    );
  }

  Future<void> _askTrust() async {
    final note = await showDialog<String>(
      context: context,
      builder: (context) => AskTrustDialog(name: _name),
    );
    if (note == null) return;
    await _do(
      () => _node.app.askTrust(identity: widget.identity, note: note),
      "Asked. You won't be told their answer.",
    );
  }

  Future<void> _edit(Profile profile) async {
    final saved = await Navigator.of(context).push<Profile>(
      MaterialPageRoute(
        builder: (_) => EditProfileScreen(node: _node, profile: profile),
      ),
    );
    if (saved == null || !mounted) return;
    setState(() => _profile = saved);
  }

  @override
  Widget build(BuildContext context) {
    final profile = _profile;
    return Scaffold(
      // Once loaded, the sky band is the bar (see [_sky]).
      appBar: profile == null
          ? SkyBar(AppBar(title: Text(_name)), joinsSky: true)
          : null,
      body: profile == null
          ? Center(
              child: _error == null
                  ? const CircularProgressIndicator()
                  : Padding(
                      padding: const EdgeInsets.all(Space.xl),
                      child: Text(
                        "Couldn't open this profile: $_error",
                        textAlign: TextAlign.center,
                      ),
                    ),
            )
          : RefreshIndicator(
              onRefresh: _reload,
              child: CustomScrollView(
                slivers: [
                  _sky(context, profile),
                  SliverToBoxAdapter(child: _header(context, profile)),
                  SliverToBoxAdapter(child: _choice(profile)),
                  ..._items(context, profile),
                ],
              ),
            ),
    );
  }

  /// Their own strip of sky, as the bar: it shrinks to the bar as the
  /// page scrolls, and stays night in both themes.
  Widget _sky(BuildContext context, Profile profile) {
    final colors = OspColors.of(context);
    final glow = profile.own
        ? colors.yourStar
        : InitialAvatar.tintFor(profile.identity);
    const r = 44.0;
    return SliverAppBar(
      pinned: true,
      expandedHeight: 200,
      backgroundColor: colors.sky,
      foregroundColor: colors.onSky,
      // Light status-bar icons on the night, on a light phone too.
      systemOverlayStyle: SystemUiOverlayStyle.light,
      title: Text(profile.own ? 'You' : shownName(profile.name)),
      actions: [
        // An OSP code for them, with words for those without OSP
        // (ADR 0039 §4).
        IconButton(
          tooltip: profile.own ? 'Share your profile' : 'Share this profile',
          onPressed: () => shareProfile(
            context,
            widget.node,
            identity: profile.identity,
            name: shownName(profile.name),
          ),
          icon: const Icon(Icons.share),
        ),
        if (!profile.own)
          PopupMenuButton<String>(
            tooltip: 'More',
            onSelected: (choice) async {
              if (choice == 'report') {
                await openReport(reportUri(account: profile.identity));
                return;
              }
              final blocked = await blockPerson(
                context,
                widget.node,
                identity: profile.identity,
                name: profile.name,
              );
              if (blocked && context.mounted) Navigator.of(context).pop();
            },
            itemBuilder: (_) => const [
              PopupMenuItem(
                value: 'block',
                child: ListTile(
                  leading: Icon(Icons.block),
                  title: Text('Block'),
                  contentPadding: EdgeInsets.zero,
                ),
              ),
              PopupMenuItem(
                value: 'report',
                child: ListTile(
                  leading: Icon(Icons.flag_outlined),
                  title: Text('Report'),
                  contentPadding: EdgeInsets.zero,
                ),
              ),
            ],
          ),
      ],
      flexibleSpace: FlexibleSpaceBar(
        collapseMode: CollapseMode.pin,
        background: Stack(
          alignment: Alignment.bottomCenter,
          children: [
            Positioned.fill(
              child: SkyBand(seed: profile.identity, glow: glow),
            ),
            // Their picture, as their star on their own sky.
            Padding(
              padding: const EdgeInsets.only(bottom: Space.m),
              child: PersonAvatar(
                node: _node,
                name: profile.name,
                identity: profile.identity,
                picture: profile.picture,
                radius: r,
              ),
            ),
          ],
        ),
      ),
    );
  }

  Widget _header(BuildContext context, Profile profile) {
    final theme = Theme.of(context);
    final quiet = theme.textTheme.bodyMedium?.copyWith(
      color: theme.colorScheme.onSurfaceVariant,
    );
    final counts = [
      if (profile.followers case final n?)
        n == 1 ? '1 follower' : '$n followers',
      if (profile.following case final n?) '$n following',
    ];
    return Padding(
      padding: const EdgeInsets.fromLTRB(Space.m, Space.m, Space.m, 0),
      child: Column(
        children: [
          Text(
            shownName(profile.name),
            style: theme.textTheme.headlineSmall,
            textAlign: TextAlign.center,
          ),
          if (counts.isNotEmpty) Text(counts.join(' · '), style: quiet),
          if (profile.known && !profile.own)
            Padding(
              padding: const EdgeInsets.only(top: Space.xs),
              child: Row(
                mainAxisSize: MainAxisSize.min,
                children: [
                  Icon(
                    Icons.handshake_outlined,
                    size: 16,
                    color: theme.colorScheme.tertiary,
                  ),
                  const SizedBox(width: Space.xs),
                  Text(
                    'You know ${shownName(profile.name)}',
                    style: quiet?.copyWith(color: theme.colorScheme.tertiary),
                  ),
                ],
              ),
            ),
          if (profile.summary case final summary?) ...[
            const SizedBox(height: Space.m),
            SelectableText(
              summary,
              style: theme.textTheme.bodyLarge,
              textAlign: TextAlign.center,
            ),
          ],
          if (profile.links.isNotEmpty) ...[
            const SizedBox(height: Space.s),
            for (final link in profile.links)
              ListTile(
                dense: true,
                contentPadding: EdgeInsets.zero,
                visualDensity: VisualDensity.compact,
                leading: Icon(Icons.link, color: theme.colorScheme.primary),
                title: Text(link.name),
                subtitle: Text(
                  link.url,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                ),
                trailing: const Icon(Icons.copy, size: 18),
                onTap: () {
                  Clipboard.setData(ClipboardData(text: link.url));
                  ScaffoldMessenger.of(
                    context,
                  ).showSnackBar(const SnackBar(content: Text('Link copied.')));
                },
              ),
          ],
          const SizedBox(height: Space.m),
          _actions(profile),
          if (!profile.own)
            TextButton.icon(
              onPressed: () => _howConnected(profile),
              icon: const Icon(Icons.route),
              label: const Text('How are we connected?'),
            ),
          if (!profile.own && !profile.followed)
            Padding(
              padding: const EdgeInsets.only(top: Space.s),
              child: Text(
                'This is what your node has found of theirs. Follow them '
                'to get everything they post from now on.',
                style: theme.textTheme.bodySmall?.copyWith(
                  color: theme.colorScheme.onSurfaceVariant,
                ),
                textAlign: TextAlign.center,
              ),
            ),
          if (_busy)
            const Padding(
              padding: EdgeInsets.only(top: Space.s),
              child: LinearProgressIndicator(),
            ),
        ],
      ),
    );
  }

  /// Asks how the owner is connected to them (ADR 0037), and shows the
  /// paths on the map. Only this tap asks; opening the profile never does.
  void _howConnected(Profile profile) {
    _node.mapWanted = true;
    _node.askConnection(widget.identity, profile.name);
    Navigator.of(context).popUntil((route) => route.isFirst);
  }

  Widget _actions(Profile profile) {
    final online = _node.reachability != Reachability.offline;
    final enabled = online && !_busy;
    final name = shownName(profile.name);
    if (profile.own) {
      return FilledButton.icon(
        onPressed: enabled ? () => _edit(profile) : null,
        icon: const Icon(Icons.edit_outlined),
        label: const Text('Edit your profile'),
      );
    }
    if (!profile.followed) {
      return FilledButton(
        onPressed: enabled ? _follow : null,
        child: const Text('Follow'),
      );
    }
    return Wrap(
      alignment: WrapAlignment.center,
      spacing: Space.s,
      runSpacing: Space.xs,
      children: [
        MenuAnchor(
          builder: (context, menu, _) => OutlinedButton.icon(
            onPressed: enabled
                ? () => menu.isOpen ? menu.close() : menu.open()
                : null,
            icon: const Icon(Icons.check),
            label: const Text('Following'),
          ),
          menuChildren: [
            MenuItemButton(
              onPressed: _unfollow,
              child: const Text('Stop following'),
            ),
          ],
        ),
        if (!profile.known)
          OutlinedButton(
            onPressed: enabled ? _trust : null,
            child: Text('I know $name'),
          ),
        OutlinedButton(
          onPressed: enabled ? _askTrust : null,
          child: Text(
            'Ask $name to confirm they know you',
            textAlign: TextAlign.center,
          ),
        ),
      ],
    );
  }

  Widget _choice(Profile profile) {
    int count(ProfileList list) => switch (list) {
      ProfileList.posts => profile.posts,
      ProfileList.media => profile.media,
      ProfileList.reshares => profile.reshares,
    };
    return Padding(
      padding: const EdgeInsets.fromLTRB(Space.m, Space.l, Space.m, Space.s),
      child: Wrap(
        alignment: WrapAlignment.center,
        spacing: Space.s,
        children: [
          for (final list in ProfileList.values)
            ChoiceChip(
              label: Text('${list.label} ${count(list)}'),
              showCheckmark: false,
              selected: _list == list,
              onSelected: (_) {
                setState(() => _list = list);
                _more();
              },
            ),
        ],
      ),
    );
  }

  List<Widget> _items(BuildContext context, Profile profile) {
    final listed = _lists[_list]!;
    final Widget end;
    if (listed.error != null) {
      end = Padding(
        padding: const EdgeInsets.all(Space.m),
        child: Column(
          children: [
            Text("Couldn't load them: ${listed.error}"),
            TextButton(onPressed: _more, child: const Text('Try again')),
          ],
        ),
      );
    } else if (listed.loading) {
      end = const Padding(
        padding: EdgeInsets.all(Space.l),
        child: Center(child: CircularProgressIndicator()),
      );
    } else if (!listed.done) {
      end = Center(
        child: TextButton(onPressed: _more, child: const Text('Show more')),
      );
    } else if (listed.posts.isEmpty) {
      end = EmptyState(
        icon: switch (_list) {
          ProfileList.posts => Icons.article_outlined,
          ProfileList.media => Icons.photo_library_outlined,
          ProfileList.reshares => Icons.repeat,
        },
        message: switch (_list) {
          ProfileList.posts => 'No posts here yet.',
          ProfileList.media => 'No photos or videos here yet.',
          ProfileList.reshares => 'No reshares here yet.',
        },
      );
    } else {
      end = const SizedBox(height: Space.l);
    }
    final Widget body;
    if (_list == ProfileList.media) {
      final tiles = [
        for (final post in listed.posts)
          for (final media in post.media) (post, media),
      ];
      body = SliverPadding(
        padding: const EdgeInsets.symmetric(horizontal: Space.xs),
        sliver: SliverGrid.count(
          crossAxisCount: 3,
          mainAxisSpacing: 2,
          crossAxisSpacing: 2,
          children: [
            for (final (post, media) in tiles)
              _GridTile(node: _node, post: post, media: media),
          ],
        ),
      );
    } else {
      body = SliverList.list(
        children: [
          for (final post in listed.posts) PostCard(node: _node, post: post),
        ],
      );
    }
    return [body, SliverToBoxAdapter(child: end)];
  }
}

/// One photo or video in a profile's grid, opened on a tap.
class _GridTile extends StatelessWidget {
  const _GridTile({
    required this.node,
    required this.post,
    required this.media,
  });

  final HomeNode node;
  final Post post;
  final Media media;

  @override
  Widget build(BuildContext context) {
    final thumb = media.thumbnailBlob;
    return InkWell(
      onTap: () => Navigator.of(context).push(
        MaterialPageRoute(
          builder: (_) => media.isVideo
              ? PlayerScreen(node: node, post: post, media: media)
              : ImageScreen(node: node, media: media),
        ),
      ),
      child: Stack(
        fit: StackFit.expand,
        children: [
          ColoredBox(
            color: Theme.of(context).colorScheme.surfaceContainerHighest,
          ),
          if (thumb != null)
            FutureBuilder<String>(
              future: node.mediaUrl(thumb, 'image/webp'),
              builder: (context, url) => url.hasData
                  ? Image.network(
                      url.data!,
                      fit: BoxFit.cover,
                      errorBuilder: (_, _, _) =>
                          const Icon(Icons.broken_image_outlined),
                    )
                  : const SizedBox.shrink(),
            ),
          if (media.isVideo)
            const Center(
              child: Icon(
                Icons.play_circle_outline,
                color: Palette.moon,
                size: 36,
              ),
            ),
        ],
      ),
    );
  }
}
