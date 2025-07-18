{
	"translatorID": "d3b1d34c-f8a1-43bb-9dd6-27aa6403b217",
	"label": "YouTube",
	"creator": "Sean Takats, Michael Berkowitz, Matt Burton, Rintze Zelle, and Geoff Banh",
	"target": "^https?://([^/]+\\.)?youtube\\.com/",
	"minVersion": "3.0",
	"maxVersion": "",
	"priority": 100,
	"inRepository": true,
	"translatorType": 4,
	"browserSupport": "gcsibv",
	"lastUpdated": "2025-06-12 16:10:09"
}

/*
	***** BEGIN LICENSE BLOCK *****

	Copyright © 2015-2024 Sean Takats, Michael Berkowitz, Matt Burton, Rintze Zelle, and Geoff Banh
	
	This file is part of Zotero.

	Zotero is free software: you can redistribute it and/or modify
	it under the terms of the GNU Affero General Public License as published by
	the Free Software Foundation, either version 3 of the License, or
	(at your option) any later version.

	Zotero is distributed in the hope that it will be useful,
	but WITHOUT ANY WARRANTY; without even the implied warranty of
	MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
	GNU Affero General Public License for more details.

	You should have received a copy of the GNU Affero General Public License
	along with Zotero. If not, see <http://www.gnu.org/licenses/>.

	***** END LICENSE BLOCK *****
*/

function detectWeb(doc, url) {
	if (/\/watch\?(?:.*)\bv=[0-9a-zA-Z_-]+/.test(url)) {
		return "videoRecording";
	}
	// Search results
	/* Testurls:
	http://www.youtube.com/user/Zoteron
	http://www.youtube.com/playlist?list=PL793CABDF042A9514
	http://www.youtube.com/results?search_query=zotero&oq=zotero&aq=f&aqi=g4&aql=&gs_sm=3&gs_upl=60204l61268l0l61445l6l5l0l0l0l0l247l617l1.2.1l4l0
	*/
	/* currently not working 2020-11-11
	if ((url.includes("/results?") || url.includes("/playlist?") || url.includes("/user/"))
			&& getSearchResults(doc, true)) {
		return "multiple";
	} */
	return false;
}

function getSearchResults(doc, checkOnly) {
	var links = doc.querySelectorAll('a.ytd-video-renderer, a.ytd-playlist-video-renderer');
	var items = {},
		found = false;
	for (var i = 0, n = links.length; i < n; i++) {
		var title = ZU.trimInternal(links[i].textContent);
		var link = links[i].href;
		if (!title || !link) continue;

		if (checkOnly) return true;

		found = true;
		items[link] = title;
	}
	return found ? items : false;
}

function doWeb(doc, url) {
	if (detectWeb(doc, url) != 'multiple') {
		scrape(doc, url);
	}
	else {
		Zotero.selectItems(getSearchResults(doc), function (items) {
			if (!items) return;

			var ids = [];
			for (var i in items) {
				ids.push(i);
			}
			ZU.processDocuments(ids, scrape);
		});
	}
}

function getMetaContent(doc, attrName, value) {
	return attr(doc, 'meta[' + attrName + '="' + value + '"]', 'content');
}

function scrape(doc, url) {
	var item = new Zotero.Item("videoRecording");
	if (!Zotero.isServer) {
		let jsonLD;
		try {
			jsonLD = JSON.parse(text(doc, 'script[type="application/ld+json"]'));
		}
		catch (e) {
			jsonLD = {};
		}

		/* YouTube won't update the meta tags for the user,
		 * if they open e.g. a suggested video in the same tab.
		 * Thus we scrape them from screen instead.
		 */

		item.title = text(doc, '#info-contents h1.title') // Desktop
			|| text(doc, '#title')
			|| text(doc, '.slim-video-information-title'); // Mobile
		// try to scrape only the canonical url, excluding additional query parameters
		item.url = url.replace(/^(.+\/watch\?v=[0-9a-zA-Z_-]+).*/, "$1").replace('m.youtube.com', 'www.youtube.com');
		item.runningTime = text(doc, '#movie_player .ytp-time-duration') // Desktop
			|| text(doc, '.ytm-time-display .time-second'); // Mobile after unmute
		if (!item.runningTime && jsonLD.duration) { // Mobile before unmute
			let duration = parseInt(jsonLD.duration.substring(2));
			let hours = String(Math.floor(duration / 3600)).padStart(2, '0');
			let minutes = String(Math.floor(duration % 3600 / 60)).padStart(2, '0');
			let seconds = String(duration % 60).padStart(2, '0');
			if (duration >= 3600) { // Include hours
				item.runningTime = `${hours}:${minutes}:${seconds}`;
			}
			else { // Just include minutes and seconds
				item.runningTime = `${minutes}:${seconds}`;
			}
		}

		item.date = ZU.strToISO(
			text(doc, '#info-strings yt-formatted-string') // Desktop
			|| attr(doc, 'ytm-factoid-renderer:last-child > div', 'aria-label') // Mobile if description has been opened
		) || jsonLD.uploadDate; // Mobile on initial page load

		var author = text(doc, '#meta-contents #text-container .ytd-channel-name') // Desktop
			|| text(doc, '#upload-info #text-container .ytd-channel-name')
			|| text(doc, '.slim-owner-channel-name'); // Mobile
		if (author) {
			item.creators.push({
				lastName: author,
				creatorType: "author",
				fieldMode: 1
			});
		}
		var description = text(doc, '#description .content')
			|| text(doc, '#description')
			|| text(doc, 'ytm-expandable-video-description-body-renderer .collapsed-string-container')
			|| text(doc, '#snippet span');
		if (description) {
			item.abstractNote = description;
		}
	}
	else {
		// required for translator server, which doesn't load the page's JS
		item.title = getMetaContent(doc, 'name', 'title');
		item.url = getMetaContent(doc, 'property', 'og:url');
		let isoDuration = getMetaContent(doc, 'itemprop', 'duration');
		// Convert ISO 8601 duration to HH:MM:SS
		item.runningTime = isoDuration.replace(/^PT/, '').replace(/H/, ':').replace(/M/, ':')
.replace(/S/, '');
		item.date = ZU.strToISO(getMetaContent(doc, 'itemprop', 'uploadDate'));
		let author = attr(doc, 'link[itemprop="name"]', 'content');
		if (author) {
			item.creators.push({
				lastName: author,
				creatorType: "author",
				fieldMode: 1
			});
		}
		let description = getMetaContent(doc, 'name', 'description');
		if (description) {
			item.abstractNote = description;
		}
	}

	item.complete();
}

/** BEGIN TEST CASES **/
var testCases = [
	{
		"type": "web",
		"url": "https://www.youtube.com/watch?v=pq94aBrc0pY",
		"defer": true,
		"items": [
			{
				"itemType": "videoRecording",
				"title": "Zotero Intro",
				"creators": [
					{
						"lastName": "Zoteron",
						"creatorType": "author",
						"fieldMode": 1
					}
				],
				"date": "2007-01-01",
				"abstractNote": "Zotero is a free, easy-to-use research tool that helps you gather and organize resources (whether bibliography or the full text of articles), and then lets you to annotate, organize, and share the results of your research. It includes the best parts of older reference manager software (like EndNote)—the ability to store full reference information in author, title, and publication fields and to export that as formatted references—and the best parts of modern software such as del.icio.us or iTunes, like the ability to sort, tag, and search in advanced ways. Using its unique ability to sense when you are viewing a book, article, or other resource on the web, Zotero will—on many major research sites—find and automatically save the full reference information for you in the correct fields.",
				"libraryCatalog": "YouTube",
				"runningTime": "2:51",
				"url": "https://www.youtube.com/watch?v=pq94aBrc0pY",
				"attachments": [],
				"tags": [],
				"notes": [],
				"seeAlso": []
			}
		]
	}
]
/** END TEST CASES **/
