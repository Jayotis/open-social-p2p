import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../format.dart';
import '../node.dart';
import '../src/rust/api/osp.dart';
import '../theme.dart';
import '../widgets/sky_bar.dart';
import 'hops_screen.dart';

/// Carrying other nodes' encrypted video (ADR 0043 §8): how it goes, every
/// control the owner has, and the question on the node's own address.
/// Opened from Your node. Each change applies at once on the node.
class CarryingScreen extends StatefulWidget {
  const CarryingScreen({super.key, required this.node});

  final HomeNode node;

  @override
  State<CarryingScreen> createState() => _CarryingScreenState();
}

class _CarryingScreenState extends State<CarryingScreen> {
  bool _busy = false;

  /// The share while its slider is held, sent when let go.
  double? _share;

  @override
  void initState() {
    super.initState();
    widget.node.checkStatus();
  }

  Future<void> _change(Future<void> Function() change) async {
    setState(() => _busy = true);
    try {
      await change();
    } catch (e) {
      if (mounted) {
        ScaffoldMessenger.of(context).showSnackBar(
          SnackBar(content: Text("Not changed: ${describeError(e)}")),
        );
      }
    } finally {
      if (mounted) {
        setState(() {
          _busy = false;
          _share = null;
        });
      }
    }
  }

  Future<void> _askNumber({
    required String title,
    required String help,
    required String unit,
    required int? current,
    required int min,
    required int max,
    required Future<void> Function(int) set,
  }) async {
    final value = await showDialog<int>(
      context: context,
      builder: (_) => _NumberDialog(
        title: title,
        help: help,
        unit: unit,
        current: current,
        min: min,
        max: max,
      ),
    );
    if (value != null) await _change(() => set(value));
  }

  @override
  Widget build(BuildContext context) {
    final node = widget.node;
    return Scaffold(
      appBar: SkyBar(AppBar(title: const Text('Carrying'))),
      body: ListenableBuilder(
        listenable: node,
        builder: (context, _) {
          final c = node.carrier;
          if (c == null) {
            return Padding(
              padding: const EdgeInsets.all(Space.m),
              child: Text(
                node.reachability == Reachability.online
                    ? "Your node doesn't say how it carries: it may need "
                          'updating.'
                    : "Your phone can't reach your node right now.",
              ),
            );
          }
          return _body(context, node, c);
        },
      ),
    );
  }

  Widget _body(BuildContext context, HomeNode node, CarrierState c) {
    final theme = Theme.of(context);
    final enabled = !_busy && node.reachability == Reachability.online;
    final share = _share ?? c.uploadShare.clamp(5, 100).toDouble();
    return ListView(
      padding: const EdgeInsets.symmetric(vertical: Space.m),
      children: [
        Padding(
          padding: const EdgeInsets.symmetric(horizontal: Space.m),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              const Text(
                'Your node carries encrypted video between other nodes, '
                'under its machine name, within the limits you set. It '
                "can't see what's inside, and it always goes behind your "
                'own traffic. Carrying well builds its standing with the '
                'nodes it helps, which can get it quicker service.',
              ),
              const SizedBox(height: Space.m),
              Text(carryingNow(c), style: theme.textTheme.titleMedium),
              if (c.bytes > 0)
                Text(
                  'It carried ${formatBytes(c.bytes)} since it started.',
                  style: theme.textTheme.bodySmall,
                ),
            ],
          ),
        ),
        const SizedBox(height: Space.s),
        SwitchListTile(
          title: const Text('Pause carrying'),
          subtitle: const Text(
            'For troubleshooting. Your node still gets every post and video.',
          ),
          value: c.paused,
          onChanged: enabled
              ? (on) => _change(() => node.setCarrying(paused: on))
              : null,
        ),
        SwitchListTile(
          title: const Text('My connection is metered'),
          subtitle: const Text('Carrying stops while this is on.'),
          value: c.metered,
          onChanged: enabled
              ? (on) => _change(() => node.setCarrying(metered: on))
              : null,
        ),
        ListTile(
          title: const Text('At once'),
          subtitle: Text(
            c.jobs == 1 ? '1 video at a time' : '${c.jobs} videos at a time',
          ),
          trailing: Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              IconButton(
                tooltip: 'Fewer',
                icon: const Icon(Icons.remove),
                onPressed: enabled && c.jobs > 1
                    ? () => _change(() => node.setCarrying(jobs: c.jobs - 1))
                    : null,
              ),
              IconButton(
                tooltip: 'More',
                icon: const Icon(Icons.add),
                onPressed: enabled && c.jobs < 100
                    ? () => _change(() => node.setCarrying(jobs: c.jobs + 1))
                    : null,
              ),
            ],
          ),
        ),
        ListTile(
          title: const Text('Share of your upload'),
          subtitle: Text(
            '${share.round()}% of ${_mbps(c.uploadMbps)}, at most',
          ),
        ),
        Padding(
          padding: const EdgeInsets.symmetric(horizontal: Space.s),
          child: Slider(
            value: share,
            min: 5,
            max: 100,
            divisions: 19,
            label: '${share.round()}%',
            onChanged: enabled ? (v) => setState(() => _share = v) : null,
            onChangeEnd: enabled
                ? (v) => _change(() => node.setCarrying(uploadShare: v.round()))
                : null,
          ),
        ),
        ListTile(
          title: const Text('Your upload speed'),
          subtitle: Text('${_mbps(c.uploadMbps)}, ${uploadFrom(c.uploadFrom)}'),
          trailing: const Icon(Icons.edit_outlined),
          enabled: enabled,
          onTap: () => _askNumber(
            title: 'Your upload speed',
            help: 'A speed test shows it; a guess is fine.',
            unit: 'Mbit/s',
            current: c.uploadFrom == 'set' ? c.uploadMbps.round() : null,
            min: 1,
            max: 100000,
            set: (v) => node.setCarrying(uploadMbps: v),
          ),
        ),
        ListTile(
          title: const Text("Your plan's monthly data cap"),
          subtitle: Text(
            c.dataCapGb == 0 ? 'Unlimited' : '${c.dataCapGb} GB a month',
          ),
          trailing: const Icon(Icons.edit_outlined),
          enabled: enabled,
          onTap: () => _askNumber(
            title: "Your plan's monthly data cap",
            help: '0 for unlimited. Carrying keeps to half of it.',
            unit: 'GB',
            current: c.dataCapGb,
            min: 0,
            max: 1000000,
            set: (v) => node.setCarrying(dataCapGb: v),
          ),
        ),
        ListTile(
          title: const Text('Carrying a month'),
          subtitle: Text('${monthlyBudget(c)}\n${monthUsed(c)}'),
          isThreeLine: true,
          trailing: const Icon(Icons.edit_outlined),
          enabled: enabled,
          onTap: () => _askNumber(
            title: 'What carrying may use a month',
            help:
                'Counting what comes in and goes out, by UTC month. 0 for no '
                'limit.',
            unit: 'GB',
            current: c.monthlyGb,
            min: 0,
            max: 1000000,
            set: (v) => node.setCarrying(monthlyGb: v),
          ),
        ),
        const Divider(height: Space.l),
        Padding(
          padding: const EdgeInsets.symmetric(horizontal: Space.m),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text('Your own video', style: theme.textTheme.titleMedium),
              const SizedBox(height: Space.s),
              const Text(hopsQuestion),
              const SizedBox(height: Space.s),
              Text(hopsAnswers, style: theme.textTheme.bodySmall),
            ],
          ),
        ),
        RadioGroup<bool>(
          groupValue: c.hops,
          onChanged: (yes) {
            if (enabled && yes != null && yes != c.hops) {
              _change(() => node.answerHops(yes));
            }
          },
          child: Column(
            children: [
              RadioListTile<bool>(
                value: true,
                enabled: enabled,
                title: const Text('Yes, go through them (recommended)'),
              ),
              RadioListTile<bool>(
                value: false,
                enabled: enabled,
                title: const Text('No, use the fallback'),
              ),
            ],
          ),
        ),
      ],
    );
  }
}

/// Whether it carries now, in a line.
String carryingNow(CarrierState c) {
  // The node's reason is in its own terms: `osp carrier` shows it.
  if (c.off != null) {
    return "It isn't carrying right now. On your node's computer, "
        "'osp carrier' says why.";
  }
  if (c.paused) return 'Carrying is paused.';
  if (c.metered) return 'Carrying is stopped while your connection is metered.';
  if (c.overBudget) {
    return "This month's carrying is used up. It carries again next month.";
  }
  if (!c.carrying) return "It isn't carrying right now.";
  return switch (c.circuits) {
    0 => 'Ready to carry. Nothing right now.',
    1 => 'Carrying 1 video now.',
    final n => 'Carrying $n videos now.',
  };
}

/// Carrying on Your node, in a line.
String carryingLine(CarrierState c) {
  if (c.off != null || c.paused || c.metered || c.overBudget) {
    return carryingNow(c);
  }
  return c.bytes == 0
      ? 'It carries encrypted video for other nodes, within your limits.'
      : 'It carried ${formatBytes(c.bytes)} of encrypted video for other '
            'nodes since it started.';
}

/// Where the upload speed came from.
String uploadFrom(String from) => switch (from) {
  'set' => 'as you set it',
  'cap' => 'from the cap on sending media',
  'measured' => 'the fastest it was seen sending',
  _ => "a slow home's, until you set yours",
};

/// What carrying may use a month.
String monthlyBudget(CarrierState c) {
  final own = c.monthlyGb;
  if (own != null) return own == 0 ? 'No limit' : 'Up to $own GB, as you set';
  if (c.dataCapGb == 0) return 'No limit';
  return "Up to ${c.dataCapGb ~/ 2} GB, half your plan's cap";
}

/// What it carried this month.
String monthUsed(CarrierState c) {
  final used = formatBytes(c.monthBytes);
  final budget = c.monthBudget;
  return budget == null
      ? '$used this month'
      : '$used of ${formatBytes(budget)} this month';
}

String _mbps(double mbps) => mbps >= 10 || mbps == mbps.roundToDouble()
    ? '${mbps.round()} Mbit/s'
    : '${mbps.toStringAsFixed(1)} Mbit/s';

class _NumberDialog extends StatefulWidget {
  const _NumberDialog({
    required this.title,
    required this.help,
    required this.unit,
    required this.current,
    required this.min,
    required this.max,
  });

  final String title;
  final String help;
  final String unit;
  final int? current;
  final int min;
  final int max;

  @override
  State<_NumberDialog> createState() => _NumberDialogState();
}

class _NumberDialogState extends State<_NumberDialog> {
  late final _text = TextEditingController(
    text: widget.current?.toString() ?? '',
  );

  int? get _value {
    final v = int.tryParse(_text.text.trim());
    return v != null && v >= widget.min && v <= widget.max ? v : null;
  }

  @override
  void dispose() {
    _text.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return AlertDialog(
      title: Text(widget.title),
      content: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(widget.help),
          TextField(
            controller: _text,
            autofocus: true,
            keyboardType: TextInputType.number,
            inputFormatters: [FilteringTextInputFormatter.digitsOnly],
            decoration: InputDecoration(suffixText: widget.unit),
            onChanged: (_) => setState(() {}),
            onSubmitted: (_) {
              if (_value != null) Navigator.pop(context, _value);
            },
          ),
        ],
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.pop(context),
          child: const Text('Cancel'),
        ),
        TextButton(
          onPressed: _value == null
              ? null
              : () => Navigator.pop(context, _value),
          child: const Text('Save'),
        ),
      ],
    );
  }
}
