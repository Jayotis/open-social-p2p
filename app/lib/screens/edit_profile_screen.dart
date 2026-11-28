import 'dart:io';

import 'package:flutter/material.dart';
import 'package:image_picker/image_picker.dart';

import '../format.dart';
import '../node.dart';
import '../src/rust/api/osp.dart';
import '../theme.dart';
import '../widgets/person_avatar.dart';
import '../widgets/sky_bar.dart';

/// Changing your profile (ADR 0032 §3): picture, name, summary and up to
/// four links. Your node publishes it; those who follow you get it on their
/// next check. Pops with the new profile once saved.
class EditProfileScreen extends StatefulWidget {
  const EditProfileScreen({
    super.key,
    required this.node,
    required this.profile,
    this.pick,
  });

  final HomeNode node;
  final Profile profile;

  /// Picks a photo and gives its path, or null if none was chosen. For
  /// tests; the phone's photo picker otherwise.
  final Future<String?> Function()? pick;

  @override
  State<EditProfileScreen> createState() => _EditProfileScreenState();
}

/// The fields of one link.
class _LinkFields {
  _LinkFields([ProfileLinkEntry? link])
    : name = TextEditingController(text: link?.name ?? ''),
      url = TextEditingController(text: link?.url ?? '');

  final TextEditingController name;
  final TextEditingController url;

  bool get blank => name.text.trim().isEmpty && url.text.trim().isEmpty;

  void dispose() {
    name.dispose();
    url.dispose();
  }
}

class _EditProfileScreenState extends State<EditProfileScreen> {
  static const _maxLinks = 4;

  late final _name = TextEditingController(text: widget.profile.name);
  late final _summary = TextEditingController(
    text: widget.profile.summary ?? '',
  );
  late final List<_LinkFields> _links = [
    for (final link in widget.profile.links) _LinkFields(link),
  ];

  /// A photo chosen on this phone, to become the picture.
  String? _photo;
  bool _removePicture = false;
  bool _saving = false;

  @override
  void dispose() {
    _name.dispose();
    _summary.dispose();
    for (final link in _links) {
      link.dispose();
    }
    super.dispose();
  }

  Future<String?> _pickPhoto() async {
    // Shrunk on the phone first, to a few hundred kilobytes; the node makes
    // the 400 px picture from it.
    final picked = await ImagePicker().pickImage(
      source: ImageSource.gallery,
      maxWidth: 1024,
      maxHeight: 1024,
      imageQuality: 90,
      requestFullMetadata: false,
    );
    return picked?.path;
  }

  Future<void> _choose() async {
    final path = await (widget.pick ?? _pickPhoto)();
    if (path == null || !mounted) return;
    setState(() {
      _photo = path;
      _removePicture = false;
    });
  }

  void _tell(String text) =>
      ScaffoldMessenger.of(context).showSnackBar(SnackBar(content: Text(text)));

  Future<void> _save() async {
    final name = _name.text.trim();
    if (name.isEmpty) {
      _tell('Your profile needs a name.');
      return;
    }
    final links = [
      for (final link in _links)
        if (!link.blank)
          ProfileLinkEntry(
            name: link.name.text.trim(),
            url: link.url.text.trim(),
          ),
    ];
    for (final link in links) {
      if (link.name.isEmpty) {
        _tell('Each link needs a name, such as "Website".');
        return;
      }
      if (!link.url.startsWith('https://')) {
        _tell('Links must start with https://');
        return;
      }
    }
    setState(() => _saving = true);
    try {
      final saved = await widget.node.app.setProfile(
        name: name,
        summary: _summary.text.trim().isEmpty ? null : _summary.text.trim(),
        links: links,
        photo: _photo,
        removePicture: _removePicture,
      );
      widget.node.profileChanged(saved);
      if (!mounted) return;
      ScaffoldMessenger.of(context).showSnackBar(
        const SnackBar(
          content: Text(
            'Saved. People who follow you see it the next time their node '
            'checks.',
          ),
        ),
      );
      Navigator.of(context).pop(saved);
    } catch (e) {
      if (mounted) {
        _tell("Couldn't save: ${describeError(e)}");
      }
    } finally {
      if (mounted) setState(() => _saving = false);
    }
  }

  Widget _picture() {
    final current = _removePicture ? null : widget.profile.picture;
    final photo = _photo;
    const radius = 48.0;
    if (photo != null) {
      return ClipOval(
        child: Image(
          image: ResizeImage(
            FileImage(File(photo)),
            width: (radius * 4).round(),
          ),
          width: radius * 2,
          height: radius * 2,
          fit: BoxFit.cover,
          errorBuilder: (_, _, _) => const CircleAvatar(
            radius: radius,
            child: Icon(Icons.broken_image_outlined),
          ),
        ),
      );
    }
    return PersonAvatar(
      node: widget.node,
      name: _name.text,
      identity: widget.profile.identity,
      picture: current,
      radius: radius,
    );
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final hasPicture =
        _photo != null || (widget.profile.picture != null && !_removePicture);
    return Scaffold(
      appBar: SkyBar(
        AppBar(
          title: const Text('Edit your profile'),
          actions: [
            TextButton(
              onPressed: _saving ? null : _save,
              child: const Text('Save'),
            ),
          ],
        ),
      ),
      body: ListView(
        padding: const EdgeInsets.all(Space.m),
        children: [
          Center(child: _picture()),
          const SizedBox(height: Space.s),
          Wrap(
            alignment: WrapAlignment.center,
            spacing: Space.s,
            children: [
              TextButton.icon(
                onPressed: _saving ? null : _choose,
                icon: const Icon(Icons.photo_outlined),
                label: Text(
                  hasPicture ? 'Choose another photo' : 'Choose a photo',
                ),
              ),
              if (hasPicture)
                TextButton(
                  onPressed: _saving
                      ? null
                      : () => setState(() {
                          _photo = null;
                          _removePicture = widget.profile.picture != null;
                        }),
                  child: const Text('No picture'),
                ),
            ],
          ),
          Text(
            'Your picture is public, like your name: anyone can see it, '
            "including people you don't know. It can show more about you than "
            'a name does.',
            style: theme.textTheme.bodySmall,
            textAlign: TextAlign.center,
          ),
          const SizedBox(height: Space.l),
          TextField(
            controller: _name,
            maxLength: 60,
            textCapitalization: TextCapitalization.words,
            decoration: const InputDecoration(labelText: 'Name'),
            onChanged: (_) => setState(() {}),
          ),
          TextField(
            controller: _summary,
            maxLength: 500,
            minLines: 2,
            maxLines: 6,
            textCapitalization: TextCapitalization.sentences,
            decoration: const InputDecoration(
              labelText: 'About you',
              hintText: 'What you post about, where you are',
            ),
          ),
          const SizedBox(height: Space.m),
          Text('Links', style: theme.textTheme.titleMedium),
          Text(
            'Up to four, such as your website. Shown as you give them.',
            style: theme.textTheme.bodySmall,
          ),
          for (final (i, link) in _links.indexed)
            Row(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Expanded(
                  flex: 2,
                  child: TextField(
                    controller: link.name,
                    maxLength: 32,
                    decoration: const InputDecoration(labelText: 'Name'),
                  ),
                ),
                const SizedBox(width: Space.s),
                Expanded(
                  flex: 3,
                  child: TextField(
                    controller: link.url,
                    maxLength: 256,
                    keyboardType: TextInputType.url,
                    autocorrect: false,
                    decoration: const InputDecoration(
                      labelText: 'Address',
                      hintText: 'https://',
                    ),
                  ),
                ),
                IconButton(
                  tooltip: 'Remove this link',
                  onPressed: _saving
                      ? null
                      : () => setState(() => _links.removeAt(i).dispose()),
                  icon: const Icon(Icons.close),
                ),
              ],
            ),
          if (_links.length < _maxLinks)
            Align(
              alignment: Alignment.centerLeft,
              child: TextButton.icon(
                onPressed: _saving
                    ? null
                    : () => setState(() => _links.add(_LinkFields())),
                icon: const Icon(Icons.add_link),
                label: const Text('Add a link'),
              ),
            ),
          if (_saving)
            const Padding(
              padding: EdgeInsets.only(top: Space.m),
              child: LinearProgressIndicator(),
            ),
        ],
      ),
    );
  }
}
