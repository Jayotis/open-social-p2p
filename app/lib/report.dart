/// Report (ADR 0042 §4): the project's report page, in the browser, with
/// what's reported filled in. Never through the network, so it works with
/// the node down and tells the reported person nothing.
library;

import 'package:url_launcher/url_launcher.dart';

const _reportPage = 'https://osp2p.com/report'; // not shown

/// The page for one post, reply or account. What's reported goes after the
/// `#`, which the browser doesn't send to the site's server: only the page
/// reads it, to fill in the form.
Uri reportUri({String? post, String? reply, String? account}) {
  final what = [
    if (post != null) 'post=${Uri.encodeComponent(post)}',
    if (reply != null) 'reply=${Uri.encodeComponent(reply)}',
    if (account != null) 'account=${Uri.encodeComponent(account)}',
  ];
  assert(what.length == 1, 'report one thing');
  return Uri.parse('$_reportPage#${what.join('&')}');
}

Future<void> openReport(Uri page) =>
    launchUrl(page, mode: LaunchMode.externalApplication);
